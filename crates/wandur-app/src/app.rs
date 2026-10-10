//! The eframe application: per frame it drains the sessions, runs the probe, draws the top bar,
//! the status bar and the dock, then applies the actions the panels asked for.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use egui_dock::{DockArea, DockState};
use wandur_core::command_line::CommandStyle;
use wandur_core::db::scripts::{self, LibraryEntry};
use wandur_core::db::{Database, DbWriter, worlds};
use wandur_core::directory::client::{Fetcher, MAX_ART_BYTES, looks_like_image, resolve_base};
use wandur_core::directory::{DirectoryService, DirectoryStatus, WorldTheme};
use wandur_core::history::{Clock, HistoryStore, SqliteHistoryStore};
use wandur_core::settings::{FileSaver, SavedWorld, Saver, Settings, unix_now};
use wandur_core::{Endpoint, Waker};

use wandur_core::l10n::{self, Language, S, plural, t, tf};

use crate::artwork::{self, ArtLoader, FetchArt, ThumbCache};
use crate::autohide::{self, AutoHide, Edge};
use crate::channels_view::ChannelsViewState;
use crate::dialogs::{Dialog, DialogResult};
use crate::directory_view::DirectoryView;
use crate::fonts::{self, FallbackFonts};
use crate::history_view::{HistoryModel, HistoryWindow};
use crate::layout;
use crate::map_view::MapViewState;
use crate::menus::{self, Command, MenuButton, MenuState, Platform};
use crate::pacer::Pacer;
use crate::panel_header;
use crate::probe::{Probe, ProbeAction, ProbeInput};
use crate::screenshot::Screenshot;
use crate::session_tab::{HistoryConfig, LoginConfig, SessionId, SessionTab, StripNotice, TabOptions};
use crate::sessions::{AppAction, Sessions};
use crate::settings_dialog::{SettingsDialog, SettingsResult};
use crate::shell::{self, Viewer};
use crate::skin::{self, Chrome, SkinId, WindowSkin};
use crate::terminal_view::TermFonts;
use crate::theme::Theme;
use crate::title_bar::{self, TitleAction, TitleMenu};
use crate::toast::{Toast, Undo};
use crate::update_notice::{NoticeAction, Updates};
use crate::widgets::{self, Icon};
use crate::workspace::{self, Tab};
use crate::workspace_panel::PanelState;
use crate::world_form::{FormResult, LibraryChange, WorldForm};
use wandur_core::directory::install::InstallHeader;
use wandur_core::login::PasswordVault;

/// Opens a web address in the browser; returns whether it opened.
pub type Launcher = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// Asks where to save a file, given a suggested name; `None` when the person cancels.
pub type ChooseFile = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// A map import scene (the Lantern Road's Mudlet map fixture brought into the first session).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapImportScene {
    /// File > Import map with the fixture read: its summary.
    Summary,
    /// Imported: the Map page on the imported area, its labels.
    Full,
    /// Imported, the player in the imported area: Play with the mini map and its labels.
    Mini,
    /// Imported: editing, the text label selected (the inspector's Label section).
    EditLabel,
}

/// How a scene shows its first session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionViewScene {
    /// The Map page: the full map fitted, the output strip under it.
    Map,
    /// Play and Map side by side.
    Split,
    /// Play, with the mini map's menu open.
    MiniMenu,
}

/// Start-up options from the command line. Values given here override the saved settings for
/// this run only.
#[derive(Clone, Default)]
pub struct Options {
    /// Endpoints to connect to at start.
    pub connect: Vec<Endpoint>,
    /// Where settings live (`--data-dir`); the platform default when `None`.
    pub data_dir: Option<PathBuf>,
    pub scrollback: Option<usize>,
    pub font_size: Option<f32>,
    pub output_fps: Option<u32>,
    pub theme: Option<String>,
    /// The window skin for this run (`--skin`, scenes), without saving it.
    pub skin: Option<String>,
    /// Let world themes apply for this run (scenes), without saving it.
    pub use_world_themes: Option<bool>,
    /// Open a title bar menu at start (scenes).
    pub title_menu: Option<crate::title_bar::TitleMenu>,
    pub directory_url: Option<String>,
    /// Open this panel at start (screenshots): `directory`, `settings`, `world:<id>`.
    pub show: Option<String>,
    /// Use this fetcher instead of HTTP (tests).
    pub fetcher: Option<Arc<dyn Fetcher>>,
    /// Fetch official maps with this instead of HTTP (tests). With a test `fetcher` and none
    /// here, official maps are never fetched.
    pub official_fetch: Option<crate::official_map::Fetch>,
    /// Save nothing (settings, layout, database writes): a scene for screenshots, set up in
    /// memory over whatever data directory it is given. Its database is an empty one of its
    /// own in a temporary folder; the data directory's `wandur.db` is never opened.
    pub ephemeral: bool,
    /// Use these saved worlds for this run instead of the saved ones (scenes).
    pub worlds: Option<Vec<SavedWorld>>,
    /// Select this saved world in the Workspace panel at start (scenes).
    pub select_world: Option<usize>,
    /// Open this menu at start (scenes), by its title.
    pub open_menu: Option<S>,
    /// Open this dialog at start (scenes): `about`, `mudlet-import` (the chooser) or
    /// `mudlet-import-summary` (with `mudlet_summary`).
    pub dialog: Option<String>,
    /// The summary text the `mudlet-import-summary` dialog shows (scenes).
    pub mudlet_summary: Option<String>,
    /// Allow Lua scripts for this run (scenes), without saving it.
    pub lua_scripts: Option<bool>,
    /// Start with this View > Layout preset instead of the saved layout (scenes).
    pub layout_preset: Option<workspace::Preset>,
    /// Change settings for this run (scenes), without saving them.
    pub adjust_settings: Option<fn(&mut wandur_core::settings::Settings)>,
    /// Draw in this language for this run, on this thread only (scenes and tests).
    pub language: Option<Language>,
    /// Open the offline demo at start.
    pub demo: bool,
    /// Opens links (tests pass a fake); the system browser otherwise.
    pub launcher: Option<Launcher>,
    /// Chooses where Save Transcript writes (tests pass a fake); a native dialog otherwise.
    pub choose_file: Option<ChooseFile>,
    /// The wandur.net base address for Help links (tests); from the environment otherwise.
    pub site: Option<String>,
    /// The first saved world's script library for this run (scenes and benches), instead of
    /// what `wandur.db` holds.
    pub macros: Option<Vec<LibraryEntry>>,
    /// Open the world editor at start (scenes): on this saved world and section.
    pub world_editor: Option<(usize, crate::world_form::Section)>,
    /// The world editor's Scripts section at start (scenes): the script selected, the output
    /// panel shown, and the completion list opened after the first `mud.` of this text.
    pub script_editor: Option<ScriptEditorScene>,
    /// Where saved passwords live (tests and scenes pass a memory vault); the system's
    /// credential store otherwise.
    pub vault: Option<Arc<dyn PasswordVault>>,
    /// Where the C# client's saved passwords are read from by File > Import from Wandur (C#)
    /// (tests pass a memory vault); its entries in the system's credential store otherwise.
    pub csharp_vault: Option<Arc<dyn PasswordVault>>,
    /// The folder File > Import from Wandur (C#) starts with (tests and scenes); the C#
    /// client's data folder otherwise.
    pub csharp_folder: Option<PathBuf>,
    /// A password already typed in the world editor's Login section (scenes).
    pub form_password: Option<String>,
    /// Typing into the last session opened, step by step, as its output arrives (scenes).
    pub scene_input: Vec<SceneInput>,
    /// Where session history goes (scenes and tests); `wandur.db` otherwise, unless ephemeral.
    pub history_store: Option<Arc<dyn HistoryStore>>,
    /// The clock history uses (tests); the system's otherwise.
    pub history_clock: Option<Clock>,
    /// Open View > Session history at start (scenes), with these filters: query, world,
    /// character, from and before dates as typed, and whether to open the first result.
    pub history_window: Option<HistoryScene>,
    /// Show the first session's full map (its Map page) set up this way once its map has rooms
    /// (scenes).
    pub map_scene: Option<crate::map_view::MapScene>,
    /// Find a MUD opens with its advanced filters shown (scenes).
    pub directory_filters_open: bool,
    /// Find a MUD opens with this select's list open: "sort" or "genre" (scenes).
    pub directory_open_select: Option<&'static str>,
    /// Show the first session this way at start (scenes): the Map page, side by side, or the
    /// mini map's menu open.
    pub session_view: Option<SessionViewScene>,
    /// The documents' and the right column's shares of the width (scenes widen the map).
    pub dock_shares: Option<(f32, f32)>,
    /// Put the menus in the macOS menu bar (a real window; never headless or in tests).
    pub native_menu: bool,
    /// Lay the window out as this platform does (tests and scenes force the Windows and Linux
    /// layout on macOS).
    pub platform: Option<Platform>,
    /// Scenes: the menu button's dropdown open (from the keyboard: the first item lit).
    pub open_menu_button: bool,
    pub open_menu_with_keyboard: bool,
    /// Scenes: open Saved worlds' Find box with this text.
    pub saved_filter: Option<String>,
    /// Scenes: fetch the directory at start (Saved worlds' pictures come from it).
    pub fetch_directory: bool,
    /// Open the first session's map editor once its map has a position, set up this way
    /// (scenes).
    pub map_editor_scene: Option<crate::map_view::MapScene>,
    /// Classify rooms with this classifier instead of an installed package (tests and scenes;
    /// it stands in for the ONNX model).
    pub room_classifier: Option<Arc<dyn wandur_core::classify::RoomClassifier>>,
    /// Replace the first session's map with these rooms, observed in order, once it has its
    /// first room (scenes).
    pub map_seed: Vec<wandur_core::map::RoomObservation>,
    /// Bring the Mudlet map fixture into the first session once it has a position (scenes).
    pub map_import_scene: Option<MapImportScene>,
    /// A session tabs scene: names, the shown tab, background tabs with new output, a toast.
    pub scene_tabs: Option<SceneTabs>,
    /// Agent profiles for this run (tests and scenes); `wandur.db` otherwise, in memory when
    /// ephemeral.
    #[cfg(feature = "agent")]
    pub agent_store: Option<Arc<dyn wandur_core::agent::AgentProfileStore>>,
    /// The model providers (tests pass fakes); the HTTP ones otherwise.
    #[cfg(feature = "agent")]
    pub agent_providers: Option<wandur_core::agent::ProviderRegistry>,
    /// The agent settings do not look models up by themselves (scenes).
    #[cfg(feature = "agent")]
    pub agent_no_discovery: bool,
    /// Set up the agent screens (scenes).
    #[cfg(feature = "agent")]
    pub agent_scene: Option<AgentScene>,
    /// Where the update check asks (tests and scenes pass a fake); `{directory}/client/latest`
    /// otherwise.
    pub update_source: Option<Arc<dyn wandur_core::updates::UpdateSource>>,
    /// The version the update check runs as (tests and scenes); this build's otherwise.
    pub update_version: Option<String>,
    /// The update check's clock (tests); the system's otherwise.
    pub update_clock: Option<wandur_core::updates::Clock>,
    /// How long after start the first automatic update check waits (20 seconds otherwise).
    pub update_delay: Option<Duration>,
}

/// How a scene sets up the agent screens.
#[cfg(feature = "agent")]
#[derive(Clone, Debug)]
pub enum AgentScene {
    /// The world editor's Agent settings on this tab.
    Settings(crate::agent_settings::EditorTab),
    /// The first session's Agent menu open, Recent decisions open with this decision, the
    /// status Paused.
    Menu(String),
}

/// The Session history window as a scene opens it.
#[derive(Clone, Debug, Default)]
pub struct HistoryScene {
    pub query: String,
    pub world: String,
    pub character: String,
    pub from: Option<std::time::SystemTime>,
    pub until: Option<std::time::SystemTime>,
    /// Open the first search result (else the first session).
    pub open_result: bool,
}

/// One scene typing step: once the last session's transcript contains `when`, put `text` in
/// its command line, and send it if `submit`.
#[derive(Clone, Debug)]
pub struct SceneInput {
    pub when: String,
    pub text: String,
    pub submit: bool,
    /// Then, once the transcript contains `after.0`, do `after.1`.
    pub after: Option<(String, SceneAfter)>,
}

/// How a scene sets up the world editor's Scripts section.
#[derive(Clone, Debug, Default)]
pub struct ScriptEditorScene {
    pub select: String,
    pub output: bool,
    pub completion_at: Option<String>,
}

/// What a scene does after its typing step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneAfter {
    /// Scroll back until the top row of the view contains this text.
    ScrollTo(String),
    /// Ctrl+click the first web address in the transcript.
    ClickLink,
    /// Show the Diagnostics page on this tab; on Messages, stop following and select the newest
    /// message with this name; on Console, show it from the top.
    Diagnostics(crate::diagnostics_view::DiagTab, Option<String>),
    /// Open View > Session history with the scene's filters.
    History,
    /// Open Mark as channel for the first transcript line that starts with this text (what
    /// the transcript menu's item does).
    MarkChannel(String),
    /// Switch this script on for the session and open the footer's Scripts menu.
    ScriptsMenu(String),
    /// Open the footer's Agent menu with Recent decisions showing this decision, Paused.
    AgentMenu(String),
}

/// What a scene waits for before it is captured.
#[derive(Clone, Debug, Default)]
pub struct SceneProbe {
    /// The Mark as channel dialog is open.
    pub mark_open: bool,
    /// File > Import from Wandur (C#) is open and has looked in its folder.
    pub csharp_import_looked: bool,
    pub catalog_worlds: usize,
    /// The directory gave up (no catalog and no fetch running).
    pub directory_failed: bool,
    /// Artwork still loading.
    pub art_pending: usize,
    /// The first session's transcript (empty without a session).
    pub session_text: String,
    /// Rooms on the first session's map.
    pub map_rooms: usize,
    /// Commands in the first session's history.
    pub history: usize,
    /// The last session opened: its transcript, whether its input is private, the typed text.
    pub last_text: String,
    pub last_private: bool,
    pub last_input: usize,
    /// The last session's ghost completion, live view rows and link waiting for Open.
    pub last_ghost: Option<String>,
    pub last_tail_rows: usize,
    pub last_pending_link: Option<String>,
    /// The last session shows the command style tip.
    pub last_style_tip: bool,
    /// What the last session's command box showed for the client's commands.
    pub last_command_help: crate::command_help::Shown,
    /// Every scene step ran.
    pub steps_done: bool,
    /// The last session's vitals cards, Diagnostics messages and server details.
    pub last_vitals: usize,
    pub last_messages: usize,
    pub last_server_details: bool,
    /// The Session history window has read its pages and opened the first item.
    pub history_ready: bool,
    /// The active session shows the notice strip.
    pub strip_shown: bool,
    /// The last session's scripts that run, and whether its Scripts menu is open.
    pub scripts_running: usize,
    pub scripts_menu: bool,
    /// The world editor's code editor shows its completion list.
    pub script_completion: bool,
    /// The world editor is open on the Scripts section with a script selected.
    pub script_selected: bool,
    /// The last session's rail: the panel titles it shows and the widgets of the open one.
    pub rail_panels: Vec<String>,
    pub rail_widgets: usize,
    /// The last session's vitals cards, by label and values (`Lantern oil 7 / 10`).
    pub vitals_text: Vec<String>,
    /// The first session's Map panel is still to be set up for a scene.
    pub map_scene_pending: bool,
    /// The last session's Agent menu is open.
    pub agent_menu: bool,
    /// Rooms of the first session's map without terrain and not yet classified.
    pub inference_pending: usize,
    /// The update strip shows.
    pub update_shown: bool,
    /// A session tabs scene has every session settled (its Lantern sessions at the prompt,
    /// its toast shown); true without such a scene.
    pub tabs_ready: bool,
}

/// A session tabs scene (`tabs-*`, `toast-*`).
#[derive(Clone, Debug, Default)]
pub struct SceneTabs {
    /// Names for the sessions, in the order they open.
    pub names: Vec<String>,
    /// The session shown (by its index).
    pub active: usize,
    /// Background sessions that keep their new output unseen; the others are marked seen.
    pub activity: Vec<usize>,
    pub toast: Option<SceneToast>,
    /// How many sessions go to The Lantern Road (the rest to closed loopback ports).
    pub lantern: usize,
}

/// The delete a toast scene makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneToast {
    /// This many rooms of the shown session's map, once it has its rooms.
    Rooms(usize),
    /// Saved world n.
    World(usize),
}

/// How often the layout is compared with the saved copy.
const LAYOUT_CHECK: Duration = Duration::from_secs(2);

pub struct WandurApp {
    dock: DockState<Tab>,
    /// Unpinned tool panels and the one slid out.
    auto: AutoHide,
    /// Where the dock was drawn in the last frame (the overlays slide out over it).
    dock_rect: egui::Rect,
    /// The toolbar world picker and, while its popup is open, the address field in it (for scenes).
    picker_rects: (egui::Rect, egui::Rect),
    /// Tool panels docked alone in their leaf, whose headers this app draws (last frame's).
    heads: HashMap<Tab, shell::Head>,
    /// The undo toast in the main window (a delete of map rooms or exits, or a saved world).
    pub toast: Option<crate::toast::Toast>,
    /// A panel drag in progress and a drop to finish (the C# drop preview and sizing).
    drop: crate::dock_drop::DropState,
    /// Strip tabs drawn this frame, for the overlay's pointer tracking.
    strip_tabs: Vec<(Tab, egui::Rect)>,
    sessions: Sessions,
    theme: Theme,
    fonts: TermFonts,
    fallback: FallbackFonts,
    pacer: Arc<Pacer>,
    settings: Settings,
    saver: Option<Saver>,
    layout_saver: Option<FileSaver>,
    saved_layout: Option<String>,
    layout_checked: Instant,
    /// Messages about settings or the layout (unreadable files, no data directory).
    notes: Vec<String>,
    address: String,
    connect_error: Option<String>,
    probe: Option<Probe>,
    screenshot: Option<Screenshot>,
    frames_presented: u64,
    waker: Waker,
    actions: Vec<AppAction>,
    title: String,
    /// Print why each frame happened (`WANDUR_REPAINT_LOG=1`), for tuning the repaint policy.
    log_repaints: bool,
    data_dir: Option<PathBuf>,
    fetcher: Arc<dyn Fetcher>,
    /// Fetches official maps (GMCP `Client.Map`).
    official_fetch: crate::official_map::Fetch,
    directory: DirectoryService,
    directory_status: DirectoryStatus,
    /// The directory setting last applied (a change restarts the directory service).
    directory_setting_seen: String,
    dir_view: DirectoryView,
    art: ArtLoader,
    panel: PanelState,
    channels: HashMap<SessionId, ChannelsViewState>,
    /// The docked Map panel's mini map, by the session it follows.
    maps: HashMap<SessionId, MapViewState>,
    /// Each session's full map (its Map page), by session.
    full_maps: HashMap<SessionId, MapViewState>,
    /// The map's share of the width side by side last settled on (new sessions start there;
    /// saved in `layout.json`).
    split_share: f32,
    /// Room terrain inference (none when this build has no classifier).
    classification: Option<Arc<wandur_core::classify::RoomClassificationService>>,
    /// Agent profiles, providers and the vault (none when this build has no agent).
    #[cfg(feature = "agent")]
    agent: Option<Arc<crate::agent_session::AgentServices>>,

    /// A scene's map editor, opened once the first session's map has a position.
    map_editor_scene: Option<crate::map_view::MapScene>,
    /// A scene's rooms for the first session's map.
    map_seed: Vec<wandur_core::map::RoomObservation>,
    map_import_scene: Option<MapImportScene>,
    /// A session tabs scene, whether it has been set up, and whether its toast was made.
    scene_tabs: Option<SceneTabs>,
    scene_tabs_started: bool,
    scene_toast_done: bool,
    active_session: Option<SessionId>,
    /// The session the world picker last followed: when another session becomes the active
    /// one, the picker (and Saved worlds' selection) moves to its world.
    picker_session: Option<SessionId>,
    directory_active: bool,
    form: Option<WorldForm>,
    /// The Mark as channel dialog, while it is open.
    mark: Option<crate::mark_channel::MarkChannel>,
    /// Script libraries (scripts and macros) by world id, read from `wandur.db` once and kept
    /// current by the world editor's saves.
    libraries: HashMap<String, Vec<LibraryEntry>>,
    requested_fetch: bool,
    panel_stats: Option<shell::PanelStats>,
    /// The client database, when it could be opened.
    db: Option<Database>,
    /// Its writer thread (none in an ephemeral run).
    db_writer: Option<DbWriter>,
    /// Loads and saves maps (none without a database, or in an ephemeral run).
    map_worker: Option<Arc<wandur_core::map::store::MapWorker>>,
    /// The saved worlds' ids and addresses last registered in the database.
    worlds_registered: Vec<(String, String, u16)>,
    /// The title bar's menu button and its dropdown (every platform; the only menu on Windows
    /// and Linux).
    menu_button: MenuButton,
    /// Alt went down with nothing else pressed since (Windows and Linux: a tap opens the menu).
    alt_tap: Option<bool>,
    /// The macOS menu bar at the top of the screen (a real window only).
    #[cfg(target_os = "macos")]
    native_menu: Option<crate::native_menu::NativeMenuBar>,
    platform: Platform,
    toolbar_visible: bool,
    dialog: Option<Dialog>,
    settings_dialog: Option<SettingsDialog>,
    /// File > Import from Wandur (C#), while open.
    csharp_import: Option<crate::csharp_import::CsharpImportDialog>,
    /// File > Import map, while open.
    pub map_import: Option<crate::map_import::MapImportDialog>,
    /// Maps imported into worlds with no session open, kept while their undo toast shows.
    detached_maps: Vec<(u64, crate::map_import::Detached)>,
    detached_serial: u64,
    /// Where that import reads the C# client's saved passwords.
    csharp_vault: Arc<dyn PasswordVault>,
    csharp_folder: Option<PathBuf>,
    launcher: Launcher,
    choose_file: Option<ChooseFile>,
    /// The text field that last had the keyboard focus (the Edit menu's target).
    edit_target: Option<egui::Id>,
    /// Edit events for the target text field, given to it next frame.
    pending_edit: Vec<egui::Event>,
    full_screen: bool,
    /// The window skin on screen, its colours, and the world theme they were made with.
    skin: SkinId,
    chrome: Chrome,
    applied_world: Option<WorldTheme>,
    /// The open title bar menu (Skin or palette).
    title_menu: Option<TitleMenu>,
    /// Where the title plate was drawn this frame (the toolbar's rim dips around it).
    title_plate: Option<egui::Rect>,
    /// Whether the OS draws the window's decorations (Windows and Linux follow the skin).
    decorations: Option<bool>,
    /// Bumped whenever `chrome` is rebuilt, so the baked frame, toolbar and plate follow it.
    chrome_generation: u64,
    /// When to put macOS's traffic lights on the drawn band again (every platform, so the
    /// schedule is tested everywhere; only macOS acts on it).
    lights_schedule: crate::traffic_lights::Scheduler,
    /// The window's full screen state as last reported, to follow changes made outside the app's
    /// own command (the green button, Esc, the Window menu).
    os_full_screen: Option<bool>,
    #[cfg(target_os = "macos")]
    lights: crate::platform::mac_traffic_lights::TrafficLights,
    baked_frame: skin::Baked,
    baked_toolbar: skin::Baked,
    baked_plate: skin::Baked,
    /// The wandur.net base address for Help links.
    site: String,
    /// Where saved passwords live.
    vault: Arc<dyn PasswordVault>,
    /// Scene typing steps still to run.
    scene_input: std::collections::VecDeque<SceneInput>,
    /// Session history: the store (none without a database or in a scene without one), its
    /// clock, the retention last applied, recorder threads still writing, and the window.
    history_store: Option<Arc<dyn HistoryStore>>,
    history_clock: Clock,
    history_applied_days: Option<u32>,
    history_drains: Vec<std::thread::JoinHandle<()>>,
    history_window: Option<HistoryWindow>,
    history_scene: Option<HistoryScene>,
    /// The install id header the HTTP fetchers add where it belongs, following the settings.
    install: InstallHeader,
    /// The update check, its schedule and the strip.
    updates: Updates,
    update_source: Option<Arc<dyn wandur_core::updates::UpdateSource>>,
    update_version: Option<String>,
    update_clock: Option<wandur_core::updates::Clock>,
    /// An ephemeral run's own database folder, removed with the app (declared last, so it goes
    /// after everything that could still be using it).
    _scratch_db: Option<ScratchDir>,
}

/// A temporary folder of its own, removed when dropped: where an ephemeral run (a scene) keeps
/// its database, so it never opens the data directory's `wandur.db` (opening can create or
/// migrate it, or move a damaged one aside).
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wandur-scene-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Accessible names for what the dock draws itself: each tab (role Tab, its title, selected
/// when it is the active one), each tab's close button, and the separators between panels.
/// egui_dock gives them ids and rectangles but no names; the ids are its own
/// (`DockArea` id, then surface, node and tab index), the separators are found by rectangle.
fn name_dock(ctx: &egui::Context, dock: &DockState<Tab>, viewer: &mut Viewer<'_>, separator: (f32, f32)) {
    use egui::accesskit::{Role, Toggled};
    use egui_dock::TabViewer as _;
    if !crate::a11y::active(ctx) {
        return;
    }
    let base = egui::Id::new("egui_dock::DockArea");
    for (path, leaf) in dock.iter_leaves() {
        for (index, tab) in leaf.tabs.iter().enumerate() {
            let id = base
                .with((path.surface, "surface"))
                .with((path.node, "node"))
                .with((index, "tab"));
            if ctx.read_response(id).is_none() {
                continue;
            }
            let title = viewer.title(&mut tab.clone()).text().to_string();
            let active = leaf.active.0 == index;
            // A tab being dragged is its own control while the drag lasts.
            let dragged = id.with("dragged");
            if ctx.read_response(dragged).is_some() {
                ctx.accesskit_node_builder(dragged, |node| {
                    node.set_role(Role::Tab);
                    node.set_label(title.as_str());
                });
            }
            ctx.accesskit_node_builder(id, |node| {
                node.set_role(Role::Tab);
                node.set_label(title.as_str());
                node.set_toggled(if active { Toggled::True } else { Toggled::False });
            });
            let close = id.with("close-button");
            if ctx.read_response(close).is_some() {
                let label = tf(S::A11yCloseTab, &[&title]);
                ctx.accesskit_node_builder(close, |node| {
                    node.set_role(Role::Button);
                    node.set_label(label.as_str());
                });
            }
        }
    }
    // A tab strip too narrow for its tabs scrolls with a thin bar under the tabs.
    for (_, leaf) in dock.iter_leaves() {
        if leaf.rect.is_positive() {
            let strip = egui::Rect::from_min_size(leaf.rect.min, egui::vec2(leaf.rect.width(), 48.0));
            crate::a11y::name_thin_inside(ctx, strip, Role::ScrollBar, t(S::A11yTabStrip));
        }
    }
    let (width, extra) = separator;
    for (_, node) in dock.iter_all_nodes() {
        let (rect, fraction, horizontal) = match node {
            egui_dock::Node::Horizontal(split) => (split.rect, split.fraction, true),
            egui_dock::Node::Vertical(split) => (split.rect, split.fraction, false),
            _ => continue,
        };
        let mut bar = rect;
        if horizontal {
            let mid = rect.min.x + rect.width() * fraction;
            bar.min.x = mid - width * 0.5;
            bar.max.x = mid + width * 0.5;
            bar = bar.expand2(egui::vec2(extra / 2.0, 0.0));
        } else {
            let mid = rect.min.y + rect.height() * fraction;
            bar.min.y = mid - width * 0.5;
            bar.max.y = mid + width * 0.5;
            bar = bar.expand2(egui::vec2(0.0, extra / 2.0));
        }
        crate::a11y::name_rect(ctx, bar, Role::Splitter, t(S::A11yResizePanels));
    }
}

/// Ask every open session's history recorder to write its pending batch now. Only a message to
/// each recorder's thread; the receivers hear when the writes are done (the History window's
/// read waits for them off the UI thread).
fn history_flushes(sessions: &mut Sessions) -> Vec<std::sync::mpsc::Receiver<()>> {
    sessions.iter_mut().filter_map(|e| e.tab.flush_history()).collect()
}

fn repaint_waker(ctx: &egui::Context) -> Waker {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

/// The artwork fetcher: the directory's HTTP client, accepting only answers that look like
/// pictures.
fn art_fetch(fetcher: Arc<dyn Fetcher>) -> FetchArt {
    Arc::new(move |url: &str| {
        let r = fetcher.get(url, MAX_ART_BYTES)?;
        if !(200..300).contains(&r.status) {
            return Err(format!("HTTP {}", r.status));
        }
        if !looks_like_image(r.content_type.as_deref(), &r.body) {
            return Err("not a picture".into());
        }
        Ok(r.body)
    })
}

impl WandurApp {
    pub fn new(ctx: &egui::Context, options: Options) -> Self {
        fonts::install(ctx);
        ctx.add_plugin(crate::a11y::RectNamer);
        let dir = options
            .data_dir
            .clone()
            .or_else(wandur_core::settings::default_data_dir);
        let (mut settings, settings_note) = match &dir {
            Some(dir) => Settings::load(dir, wandur_term::MAX_SCROLLBACK),
            None => (Settings::default(), Some(t(S::NoDataDirectory).into())),
        };
        let saver = dir.clone().filter(|_| !options.ephemeral).map(Saver::new);
        let mut notes: Vec<String> = settings_note.into_iter().collect();
        // A scene saves nothing and reads nothing of the person's database: it gets an empty one
        // of its own instead.
        let scratch_db = options.ephemeral.then(ScratchDir::new);
        let db_dir = match &scratch_db {
            Some(scratch) => Some(scratch.0.clone()),
            None => dir.clone(),
        };
        let db = match db_dir.as_deref().map(Database::open) {
            Some(Ok((db, note))) => {
                notes.extend(note);
                Some(db)
            }
            Some(Err(e)) => {
                notes.push(tf(S::DatabaseNotInUse, &[&e]));
                None
            }
            None => None,
        };
        let db_writer = match (&db, options.ephemeral) {
            (Some(db), false) => match db.writer() {
                Ok(w) => Some(w),
                Err(e) => {
                    notes.push(tf(S::DatabaseNotWritable, &[&e]));
                    None
                }
            },
            _ => None,
        };
        // Command line values apply to this run without being saved.
        if let Some(v) = options.scrollback {
            settings.scrollback = v;
        }
        if let Some(v) = options.font_size {
            settings.font_size = v;
        }
        if let Some(v) = options.output_fps {
            settings.output_fps = v;
        }
        if let Some(v) = &options.theme {
            settings.theme = v.clone();
        }
        if let Some(v) = &options.skin {
            settings.skin = v.clone();
        }
        if let Some(v) = options.use_world_themes {
            settings.use_world_themes = v;
        }
        if let Some(v) = options.lua_scripts {
            settings.enable_lua_scripts = v;
        }
        if let Some(adjust) = options.adjust_settings {
            adjust(&mut settings);
        }
        if let Some(worlds) = &options.worlds {
            settings.worlds = worlds.clone();
        }
        settings.clamp(wandur_term::MAX_SCROLLBACK);
        match options.language {
            Some(language) => {
                l10n::override_thread(Some(language));
                // A scene shows its language chosen in Settings, as the C# captures do.
                if options.ephemeral {
                    settings.language = language.code().into();
                }
            }
            None => crate::settings_dialog::apply_language(&settings.language),
        }
        let theme = Theme::from_settings(&settings);
        theme.apply(ctx);

        let (mut dock, mut auto, layout_note) = match &dir {
            // A new install starts with Panels on the left; scenes keep the C# layout their
            // reference captures have.
            Some(dir) => layout::load(
                dir,
                if options.ephemeral {
                    workspace::Preset::Both
                } else {
                    workspace::NEW_INSTALL
                },
            ),
            None => (workspace::default_layout(), AutoHide::default(), None),
        };
        notes.extend(layout_note);
        if let Some((documents, right)) = options.dock_shares {
            dock = workspace::layout_with_shares(documents, right);
        }
        if let Some(preset) = options.layout_preset {
            (dock, auto) = workspace::preset_layout(preset);
        }
        let saved_split = dir
            .as_deref()
            .and_then(layout::load_split_share)
            .unwrap_or(crate::terminal_view::DEFAULT_SPLIT_SHARE);
        let saved_layout = layout::to_json_with_split(&dock, &auto, chosen_split(saved_split));
        let layout_saver = dir
            .as_ref()
            .filter(|_| !options.ephemeral)
            .map(|d| FileSaver::new(d.join(layout::LAYOUT_FILE)));

        let pacer = Arc::new(Pacer::new(settings.output_fps));
        let waker: Waker = {
            let pacer = Arc::clone(&pacer);
            let ctx = ctx.clone();
            Arc::new(move || pacer.wake(&ctx))
        };
        // WANDUR_DIRECTORY_URL, then --directory-url (this run only), then the setting, then wandur.net.
        let directory_setting = options
            .directory_url
            .clone()
            .unwrap_or_else(|| settings.directory_url.clone());
        let install = InstallHeader::new(
            &resolve_base(
                std::env::var("WANDUR_DIRECTORY_URL").ok().as_deref(),
                &directory_setting,
            ),
            &settings,
        );
        let fetcher: Arc<dyn Fetcher> = match options.fetcher.clone() {
            Some(f) => f,
            None => default_fetcher(&install),
        };
        let official_fetch = match (&options.official_fetch, &options.fetcher) {
            (Some(f), _) => Arc::clone(f),
            // A test run never goes out to the network.
            (None, Some(_)) => {
                let offline: crate::official_map::Fetch =
                    Arc::new(|_: &str, _: &wandur_core::map::official::store::Validators| {
                        Err(wandur_core::map::official::download::DownloadError::Network(
                            "offline".into(),
                        ))
                    });
                offline
            }
            (None, None) => crate::official_map::default_fetch(),
        };
        // An ephemeral run keeps no offline copy of what it fetched, and no thumbnails.
        let cache_dir = dir.clone().filter(|_| !options.ephemeral);
        let directory = start_directory(&directory_setting, cache_dir.as_deref(), &fetcher, ctx);
        let directory_setting_seen = settings.directory_url.clone();
        let thumbs = cache_dir
            .as_ref()
            .map(|d| ThumbCache::open(d.join("thumbnails"), artwork::disk::DEFAULT_LIMIT));
        let art = ArtLoader::new(
            artwork::DEFAULT_WORKERS,
            art_fetch(Arc::clone(&fetcher)),
            thumbs,
            artwork::DEFAULT_BUDGET,
            repaint_waker(ctx),
        );
        let probe = Probe::from_env(ctx);
        let map_worker = db
            .as_ref()
            .filter(|_| !options.ephemeral)
            .and_then(|db| {
                wandur_core::map::store::MapWorker::spawn(wandur_core::map::store::MapStore::new(db.clone())).ok()
            })
            .map(Arc::new);
        let history_store: Option<Arc<dyn HistoryStore>> = match (&options.history_store, &db) {
            (Some(store), _) => Some(Arc::clone(store)),
            (None, Some(db)) if !options.ephemeral => Some(Arc::new(SqliteHistoryStore::new(db.clone()))),
            _ => None,
        };
        let classification = classification_service(&options, dir.as_deref());
        let updates = Updates::new(
            update_service(
                options.update_source.as_ref(),
                options.update_version.as_deref(),
                options.update_clock.as_ref(),
                directory.base(),
                &install,
            ),
            &settings,
            options.update_delay.unwrap_or(crate::update_notice::STARTUP_DELAY),
        );
        // The install id is made on the first load; keep it from then on (not in a scene).
        let keep_install_id = !options.ephemeral
            && dir.as_deref().is_some_and(|d| {
                wandur_core::directory::install::stored_id(&d.join(wandur_core::settings::SETTINGS_FILE)).is_none()
            });
        let settings_skin = settings.skin.clone();
        let theme_for_chrome = theme.clone();
        let mut app = Self {
            dock,
            auto,
            dock_rect: egui::Rect::NOTHING,
            picker_rects: (egui::Rect::NOTHING, egui::Rect::NOTHING),
            heads: HashMap::new(),
            toast: None,
            drop: Default::default(),
            strip_tabs: Vec::new(),
            sessions: Sessions::default(),
            fonts: TermFonts::new(settings.font_size),
            fallback: FallbackFonts::new(),
            pacer,
            theme,
            address: options.connect.first().map(ToString::to_string).unwrap_or_default(),
            settings,
            saver,
            layout_saver,
            saved_layout,
            layout_checked: Instant::now(),
            notes,
            connect_error: None,
            probe,
            screenshot: Screenshot::from_env(),
            frames_presented: 0,
            waker,
            actions: Vec::new(),
            title: String::new(),
            log_repaints: std::env::var_os("WANDUR_REPAINT_LOG").is_some(),
            data_dir: dir,
            fetcher,
            official_fetch,
            directory_status: DirectoryStatus::default(),
            directory_setting_seen,
            directory,
            dir_view: DirectoryView::default(),
            art,
            panel: PanelState::default(),
            channels: HashMap::new(),
            maps: HashMap::new(),
            full_maps: HashMap::new(),
            split_share: saved_split,
            classification,
            #[cfg(feature = "agent")]
            agent: None,

            map_editor_scene: options.map_editor_scene.clone(),
            map_seed: options.map_seed.clone(),
            map_import_scene: options.map_import_scene,
            scene_tabs: options.scene_tabs.clone(),
            scene_tabs_started: false,
            scene_toast_done: false,
            active_session: None,
            picker_session: None,
            directory_active: false,
            form: None,
            mark: None,
            libraries: HashMap::new(),
            requested_fetch: false,
            panel_stats: None,
            db,
            db_writer,
            map_worker,
            worlds_registered: Vec::new(),
            menu_button: MenuButton::default(),
            alt_tap: None,
            #[cfg(target_os = "macos")]
            native_menu: options.native_menu.then(|| crate::native_menu::NativeMenuBar::new(ctx)),
            platform: options.platform.unwrap_or_else(Platform::current),
            toolbar_visible: true,
            dialog: None,
            settings_dialog: None,
            csharp_import: None,
            map_import: None,
            detached_maps: Vec::new(),
            detached_serial: 0,
            csharp_vault: match &options.csharp_vault {
                Some(vault) => Arc::clone(vault),
                None if options.ephemeral => Arc::new(wandur_core::login::MemoryVault::new()),
                None => wandur_core::import::csharp::secrets::csharp_vault(),
            },
            csharp_folder: options.csharp_folder.clone(),
            launcher: options.launcher.clone().unwrap_or_else(default_launcher),
            choose_file: options.choose_file.clone(),
            edit_target: None,
            pending_edit: Vec::new(),
            full_screen: false,
            skin: SkinId::from_name(&settings_skin),
            chrome: Chrome::new(&theme_for_chrome, SkinId::from_name(&settings_skin), None),
            applied_world: None,
            title_menu: options.title_menu,
            title_plate: None,
            decorations: None,
            chrome_generation: 0,
            lights_schedule: Default::default(),
            os_full_screen: None,
            #[cfg(target_os = "macos")]
            lights: Default::default(),
            baked_frame: skin::Baked::default(),
            baked_toolbar: skin::Baked::default(),
            baked_plate: skin::Baked::default(),
            site: options.site.clone().unwrap_or_else(wandur_core::site::base),
            // A scene saves nothing, so it keeps passwords in memory too.
            vault: match &options.vault {
                Some(vault) => Arc::clone(vault),
                None if options.ephemeral => Arc::new(wandur_core::login::MemoryVault::named(
                    wandur_core::login::vault::system().name(),
                )),
                None => wandur_core::login::vault::system(),
            },
            scene_input: options.scene_input.iter().cloned().collect(),
            history_store,
            history_clock: options
                .history_clock
                .clone()
                .unwrap_or_else(wandur_core::history::system_clock),
            history_applied_days: None,
            history_drains: Vec::new(),
            history_window: None,
            history_scene: options.history_window.clone(),
            install,
            updates,
            update_source: options.update_source.clone(),
            update_version: options.update_version.clone(),
            update_clock: options.update_clock.clone(),
            _scratch_db: scratch_db,
        };
        if keep_install_id {
            app.save_settings();
        }
        // Retention runs at start, off the UI thread.
        app.apply_history_retention();
        if app.assign_world_ids() {
            app.save_settings();
        } else {
            app.register_worlds();
        }
        if let (Some(library), Some(world)) = (options.macros.clone(), app.settings.worlds.first()) {
            app.libraries.insert(world.world_id.clone(), library);
        }
        #[cfg(feature = "agent")]
        {
            use wandur_core::agent::{
                AgentProfileStore, MemoryAgentProfileStore, ProviderRegistry, SqliteAgentProfileStore,
            };
            let store: Arc<dyn AgentProfileStore> = match (&options.agent_store, &app.db) {
                (Some(store), _) => Arc::clone(store),
                (None, Some(db)) if !options.ephemeral => Arc::new(SqliteAgentProfileStore::new(db.clone())),
                _ => Arc::new(MemoryAgentProfileStore::new()),
            };
            app.agent = Some(Arc::new(crate::agent_session::AgentServices {
                store,
                providers: options
                    .agent_providers
                    .clone()
                    .unwrap_or_else(ProviderRegistry::standard),
                vault: Arc::clone(&app.vault),
                discover: !options.agent_no_discovery,
            }));
        }
        for endpoint in options.connect {
            let world = app.settings.worlds.iter().position(|w| w.is_at(&endpoint));
            app.open(endpoint, world);
        }
        let first = app.sessions.iter().next().map(|e| e.tab.id);
        if let Some(id) = first {
            if let Some(scene) = options.map_scene.clone() {
                app.full_maps
                    .entry(id)
                    .or_insert_with(MapViewState::full)
                    .set_scene(scene);
                if let Some(entry) = app.sessions.get_mut(id) {
                    entry.view.show_map(true);
                }
            }
            match options.session_view {
                Some(SessionViewScene::Map | SessionViewScene::Split) => {
                    let split = options.session_view == Some(SessionViewScene::Split);
                    let state = app.full_maps.entry(id).or_insert_with(MapViewState::full);
                    if !state.scene_pending() {
                        state.set_scene(crate::map_view::MapScene::Fit);
                    }
                    if let Some(entry) = app.sessions.get_mut(id) {
                        entry.view.show_map(true);
                        entry.view.set_split(split);
                    }
                }
                Some(SessionViewScene::MiniMenu) => {
                    app.maps.entry(id).or_insert_with(MapViewState::mini).tools_open = true;
                }
                None => {}
            }
        }
        if options.demo {
            app.open_demo();
        }
        if let Some(show) = options.show.as_deref() {
            app.show_at_start(show);
        }
        app.panel.selected_world = options.select_world.filter(|&i| i < app.settings.worlds.len());
        if options.fetch_directory {
            app.dir_view.wants_fetch = true;
        }
        app.dir_view.set_filters_open(options.directory_filters_open);
        app.dir_view.open_select = options.directory_open_select;
        if let Some(text) = &options.saved_filter {
            app.panel.filter_open = true;
            app.panel.filter = text.clone();
        }
        if let Some(menu) = options.open_menu {
            let entries = menus::button_entries(&menus::menus(app.platform, &app.menu_state()));
            app.menu_button.open_menu(&entries, menu);
        }
        if options.open_menu_button {
            let entries = menus::button_entries(&menus::menus(app.platform, &app.menu_state()));
            if options.open_menu_with_keyboard {
                app.menu_button.open_with_keyboard(&entries);
            } else {
                app.menu_button.toggle();
            }
        }
        match options.dialog.as_deref() {
            Some("about") => app.dialog = Some(Dialog::About),
            Some("mudlet-import") => app.open_mudlet_import(),
            Some("csharp-import") => app.open_csharp_import(ctx),
            Some("mudlet-import-summary") => {
                app.dialog = Some(Dialog::MudletSummary(
                    options.mudlet_summary.clone().unwrap_or_default(),
                ));
            }
            _ => {}
        }
        if let Some((index, section)) = options.world_editor {
            app.edit_world(Some(index));
            if let Some(form) = &mut app.form {
                form.section = section;
                #[cfg(feature = "agent")]
                if let (Some(AgentScene::Settings(tab)), Some(agent)) = (&options.agent_scene, &mut form.agent) {
                    agent.tab = *tab;
                }
                if let Some(password) = &options.form_password {
                    form.password = password.clone();
                }
                if let Some(scene) = &options.script_editor {
                    let found = form
                        .macros
                        .scripts()
                        .find(|e| e.name == scene.select)
                        .map(|e| e.id.clone());
                    if let Some(id) = found {
                        form.macros.select_script(&id);
                    }
                    form.macros.show_output = scene.output;
                    // The caret is placed in the text the editor shows (formatted).
                    if let Some(after) = &scene.completion_at
                        && let Some(source) = form
                            .macros
                            .selected_script_entry()
                            .map(|e| crate::script_editor::shown_text(&e.source, e.is_lua()))
                        && let Some(at) = source.find(after.as_str())
                    {
                        let caret = source[..at].chars().count() + "mud.".len();
                        form.macros.editor.set_caret = Some(caret);
                        form.macros.editor.request_completion = true;
                    }
                }
            }
        }
        app
    }

    /// A world's script library: from the cache, else read from `wandur.db` (a short read).
    fn library(&mut self, world_id: &str) -> Vec<LibraryEntry> {
        if world_id.is_empty() {
            return Vec::new();
        }
        if let Some(library) = self.libraries.get(world_id) {
            return library.clone();
        }
        let mut library = match &self.db {
            Some(db) => match db.read(|c| Ok((scripts::load(c, world_id)?, scripts::is_started(c, world_id)?))) {
                Ok((mut library, started)) => {
                    // The first time a world's library is opened empty it gets the starter
                    // script (C#); the writer adds it unless something got there first.
                    if !started && worlds::valid_world_id(world_id) {
                        let starter = scripts::starter();
                        if library.is_empty() {
                            library.push(starter.clone());
                        }
                        if let Some(writer) = &self.db_writer {
                            let id = world_id.to_string();
                            writer.submit(Box::new(move |c| scripts::ensure_started(c, &id, &starter).map(|_| ())));
                        }
                    }
                    library
                }
                Err(e) => {
                    self.notes.insert(0, tf(S::ScriptLoadFailed, &[&e]));
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        library.truncate(scripts::MAX_ENTRIES);
        self.libraries.insert(world_id.to_string(), library.clone());
        library
    }

    /// Read a world's library again from `wandur.db` (Scripts > Reload saved rules).
    fn reload_library(&mut self, world_id: &str) -> Vec<LibraryEntry> {
        if self.db.is_some() {
            self.libraries.remove(world_id);
        }
        self.library(world_id)
    }

    /// The world id a session's macros belong to: its saved world's, else the one `wandur.db`
    /// has for the address.
    fn session_world_id(&self, endpoint: &Endpoint, world: Option<usize>) -> Option<String> {
        if let Some(w) = world.and_then(|i| self.settings.worlds.get(i))
            && worlds::valid_world_id(&w.world_id)
        {
            return Some(w.world_id.clone());
        }
        let db = self.db.as_ref()?;
        db.read(|c| worlds::find_world(c, &endpoint.host, endpoint.port))
            .ok()
            .flatten()
    }

    /// The world a Mudlet package goes into: the shown session's saved world, else the one
    /// selected in the Workspace list (the C# world picker's selection).
    fn mudlet_target(&self) -> Option<usize> {
        self.active_session
            .filter(|_| !self.directory_active)
            .and_then(|id| self.sessions.get(id))
            .and_then(|e| e.tab.world)
            .or(self.panel.selected_world)
            .filter(|&i| i < self.settings.worlds.len())
    }

    /// File > Import map: the dialog, for `session`'s world (else the active session's; else
    /// the first world listed). Open sessions come first, then saved worlds with none open.
    pub fn open_map_import(&mut self, ctx: &egui::Context, session: Option<SessionId>) {
        use crate::map_import::{MapImportDialog, Target, WorldChoice};
        let preferred = session.or(self.active_session);
        let mut choices = Vec::new();
        let mut chosen = 0;
        let mut open_worlds = Vec::new();
        for entry in self.sessions.iter() {
            if Some(entry.tab.id) == preferred {
                chosen = choices.len();
            }
            choices.push(WorldChoice {
                label: tf(S::MapImportOpenSession, &[&entry.tab.title()]),
                target: Target::Session(entry.tab.id),
            });
            if let Some(world) = entry.tab.map_world() {
                open_worlds.push(world.clone());
            }
        }
        if self.db.is_some() {
            for world in &self.settings.worlds {
                if !worlds::valid_world_id(&world.world_id) {
                    continue;
                }
                let key = wandur_core::map::store::MapWorld::Id(world.world_id.clone());
                if open_worlds.contains(&key) {
                    continue;
                }
                choices.push(WorldChoice {
                    label: world.name.clone(),
                    target: Target::World(key),
                });
            }
        }
        self.map_import = Some(MapImportDialog::new(choices, chosen, repaint_waker(ctx)));
    }

    /// Show File > Import map and act on it.
    fn show_map_import(&mut self, ctx: &egui::Context) {
        use crate::map_import::ImportAction;
        let Some(dialog) = &mut self.map_import else { return };
        if let Some(finished) = dialog.take_finished() {
            let label = dialog
                .choices
                .get(dialog.chosen)
                .map(|c| c.label.clone())
                .unwrap_or_default();
            self.map_import = None;
            self.finish_detached_import(finished, &label);
            return;
        }
        match dialog.show(ctx, &self.theme) {
            ImportAction::Open => {}
            ImportAction::Closed => self.map_import = None,
            ImportAction::ChooseFile => {
                if let Some(path) = crate::map_import::pick_file()
                    && let Some(dialog) = &mut self.map_import
                {
                    dialog.read_file(&path);
                }
            }
            ImportAction::Apply(prepared, target) => self.apply_map_import(*prepared, target),
        }
    }

    /// Import's button: into a session's live map at once, or into a world's saved map on a
    /// worker thread (the dialog shows progress until it is done).
    pub fn apply_map_import(
        &mut self,
        prepared: wandur_core::map::mudlet::Prepared,
        target: crate::map_import::Target,
    ) {
        use crate::map_import::Target;
        match target {
            Target::Session(id) => {
                self.map_import = None;
                self.import_into_session(id, &prepared);
            }
            Target::World(world) => {
                let Some(db) = self.db.clone() else {
                    self.map_import = None;
                    self.notes
                        .insert(0, tf(S::MapImportFailed, &[&t(S::DatabaseOpenFailed)]));
                    return;
                };
                // Saves still queued land first, so the import merges with them.
                if let Some(maps) = &self.map_worker {
                    maps.flush();
                }
                if let Some(dialog) = &mut self.map_import {
                    dialog.apply_detached(db, world, prepared);
                }
            }
        }
    }

    /// Apply a read map to a session's live map as one undoable step; the undo toast offers
    /// it back and the session's Map page shows it.
    pub fn import_into_session(&mut self, id: SessionId, prepared: &wandur_core::map::mudlet::Prepared) {
        let Some(entry) = self.sessions.get_mut(id) else {
            return;
        };
        entry.tab.stop_walk();
        let result = crate::map_import::apply_to(entry.tab.map.tracker_mut(), prepared);
        for state in [self.full_maps.get_mut(&id), self.maps.get_mut(&id)]
            .into_iter()
            .flatten()
        {
            crate::map_view::editor::reset_after_import(state);
        }
        if let Some(inference) = &mut entry.tab.inference {
            inference.rescan();
        }
        match result {
            Ok(0) => self.notes.insert(0, t(S::MapImportNothingNew).to_string()),
            Ok(_) => {
                let edit = entry.tab.map.tracker().last_edit();
                let text = tf(S::ToastImportedMap, &[&entry.tab.title()]);
                if let Some(edit) = edit {
                    self.set_toast(Toast::new(text, Undo::Map { session: id, edit }));
                }
                self.actions.push(AppAction::OpenFullMap(id));
            }
            Err(e) => self.notes.insert(0, tf(S::MapImportFailed, &[&e])),
        }
    }

    /// An import into a world with no session finished: the toast offers it back.
    fn finish_detached_import(&mut self, result: Result<crate::map_import::Detached, String>, label: &str) {
        match result {
            Ok(done) if done.changed == 0 => self.notes.insert(0, t(S::MapImportNothingNew).to_string()),
            Ok(done) => {
                self.detached_serial += 1;
                let key = self.detached_serial;
                if let Some(edit) = done.tracker.last_edit() {
                    self.detached_maps.push((key, done));
                    self.set_toast(Toast::new(
                        tf(S::ToastImportedMap, &[&label]),
                        Undo::DetachedMap { key, edit },
                    ));
                }
            }
            Err(e) => self.notes.insert(0, tf(S::MapImportFailed, &[&e])),
        }
    }

    /// File > Import from Wandur (C#): the dialog, already looking in the C# client's folder.
    fn open_csharp_import(&mut self, ctx: &egui::Context) {
        if self.csharp_import.is_none() {
            self.csharp_import = Some(crate::csharp_import::CsharpImportDialog::new(
                self.csharp_folder.clone(),
                repaint_waker(ctx),
            ));
        }
    }

    /// Show File > Import from Wandur (C#) and act on it.
    fn show_csharp_import(&mut self, ctx: &egui::Context) {
        use crate::csharp_import::ImportAction;
        let Some(dialog) = &mut self.csharp_import else { return };
        match dialog.show(ctx, &self.theme) {
            ImportAction::Open => {}
            ImportAction::Closed => self.csharp_import = None,
            ImportAction::ChooseFolder => {
                let start = PathBuf::from(dialog.folder.trim());
                if let Some(folder) = crate::csharp_import::pick_folder(&start) {
                    dialog.folder = folder.display().to_string();
                    dialog.look();
                }
            }
            ImportAction::Start => self.start_csharp_import(),
            ImportAction::Apply(settings) => self.apply_csharp_import(ctx, *settings),
        }
    }

    /// The dialog's Import: hand the worker this client's database, settings and vaults.
    fn start_csharp_import(&mut self) {
        let Some(dialog) = &mut self.csharp_import else { return };
        let Some(db) = self.db.clone() else {
            self.csharp_import = None;
            self.notes
                .insert(0, tf(S::CsImportFailed, &[&t(S::DatabaseOpenFailed)]));
            return;
        };
        // Everything queued for the database is written first, so the import merges with it.
        if let Some(writer) = &self.db_writer {
            writer.flush();
        }
        if let Some(maps) = &self.map_worker {
            maps.flush();
        }
        dialog.start(
            db,
            self.settings.clone(),
            Arc::clone(&self.csharp_vault),
            Arc::clone(&self.vault),
        );
    }

    /// Take the imported worlds and preferences. The import appends worlds and updates others in
    /// place, so open sessions keep pointing at their world. Libraries are read again.
    fn apply_csharp_import(&mut self, ctx: &egui::Context, imported: Settings) {
        self.libraries.clear();
        self.save_preferences(ctx, imported);
    }

    /// File > Import from Mudlet: the chooser.
    fn open_mudlet_import(&mut self) {
        let target = self.mudlet_target().map(|i| self.settings.worlds[i].name.clone());
        self.dialog = Some(Dialog::MudletChooser { target });
    }

    /// Import what the person picked: read it, pick or add the world, merge the converted
    /// scripts into its library, give open sessions of that world the new library, and show
    /// the summary. A failure is a notice, as in C#.
    #[doc(hidden)]
    pub fn import_mudlet(&mut self, path: &std::path::Path) {
        use wandur_core::mudlet::{importer, source};
        let selected = self.mudlet_target();
        let read = source::read(path).and_then(|read| {
            let target = importer::resolve_world(&read, &self.settings.worlds, selected)?;
            Ok((read, target))
        });
        let (read, target) = match read {
            Ok(found) => found,
            Err(error) => {
                self.notes.insert(0, tf(S::MudletImportFailed, &[&error]));
                return;
            }
        };
        let created = target.created();
        let mut world = target.world;
        if !worlds::valid_world_id(&world.world_id) {
            world.world_id = self
                .db
                .as_ref()
                .and_then(|db| {
                    db.read(|c| worlds::find_world(c, &world.host, world.port))
                        .ok()
                        .flatten()
                })
                .unwrap_or_else(worlds::new_world_id);
        }
        let unchanged = target.index.and_then(|i| self.settings.worlds.get(i)) == Some(&world);
        if !unchanged {
            self.save_world(target.index, world.clone());
        }
        let before = self.library(&world.world_id);
        let (after, summary) = importer::apply(&read, &world.world_id, &world.name, created, &before);
        self.save_library(&world.world_id, LibraryChange { before, after });
        // Open sessions on that world pick the new scripts up at once.
        let open: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|e| e.tab.world_id.as_deref() == Some(world.world_id.as_str()))
            .map(|e| e.tab.id)
            .collect();
        for id in open {
            self.attach_library(id, false);
        }
        self.dialog = Some(Dialog::MudletSummary(summary.to_text(5)));
    }

    /// Give a session its world's macros and scripts.
    fn attach_macros(&mut self, id: SessionId) {
        self.attach_library(id, false);
    }

    /// Give a session its world's macros and scripts, read again from `wandur.db` when
    /// `reload` (the Scripts menu's Reload saved rules).
    fn attach_library(&mut self, id: SessionId, reload: bool) {
        let Some(entry) = self.sessions.get(id) else { return };
        let endpoint = entry.tab.endpoint.clone();
        let world_id = self.session_world_id(&endpoint, entry.tab.world);
        let library = match world_id.as_deref() {
            Some(w) if reload => self.reload_library(w),
            Some(w) => self.library(w),
            None => Vec::new(),
        };
        let macros = library.iter().filter_map(LibraryEntry::as_saved_macro).collect();
        let scripts = script_definitions(&library);
        if let Some(entry) = self.sessions.get_mut(id) {
            let now = Instant::now();
            entry.tab.set_macros(world_id, macros, now);
            entry.tab.set_scripts(scripts, now);
        }
    }

    /// Give a session its world's agent (profile, goals, a runner).
    fn attach_agent(&mut self, id: SessionId) {
        #[cfg(feature = "agent")]
        {
            let Some(services) = self.agent.clone() else { return };
            let Some(world) = self
                .sessions
                .get(id)
                .map(|e| self.agent_world(&e.tab.endpoint, e.tab.world, e.tab.is_demo()))
            else {
                return;
            };
            if let Some(entry) = self.sessions.get_mut(id) {
                entry.tab.configure_agent(services, world);
            }
        }
        #[cfg(not(feature = "agent"))]
        let _ = id;
    }

    /// The world a session's or a saved world's agent profile belongs to.
    #[cfg(feature = "agent")]
    fn agent_world(&self, endpoint: &Endpoint, world: Option<usize>, demo: bool) -> wandur_core::agent::AgentWorld {
        use wandur_core::agent::AgentWorld;
        if demo {
            return AgentWorld::demo();
        }
        match self.session_world_id(endpoint, world) {
            Some(id) => AgentWorld::Id(id),
            None => AgentWorld::endpoint(&endpoint.host, endpoint.port),
        }
    }

    /// The world editor's Agent settings section for the session's saved world (Configure
    /// agent...).
    fn edit_agent(&mut self, id: SessionId) {
        let Some(index) = self.sessions.get(id).and_then(|e| e.tab.world) else {
            return;
        };
        if index >= self.settings.worlds.len() {
            return;
        }
        self.edit_world(Some(index));
        if let Some(form) = &mut self.form {
            form.section = crate::world_form::Section::Agent;
        }
    }

    /// Save the world editor's agent settings for the saved world at `index` and give open
    /// sessions of that world the new goals (their runs stop and their memory is cleared).
    #[cfg(feature = "agent")]
    fn save_agent_settings(&mut self, index: usize) -> Result<(), String> {
        let Some(world) = self.settings.worlds.get(index) else {
            return Ok(());
        };
        let world_id = world.world_id.clone();
        let target = if worlds::valid_world_id(&world_id) {
            wandur_core::agent::AgentWorld::Id(world_id.clone())
        } else {
            wandur_core::agent::AgentWorld::endpoint(&world.host, world.port)
        };
        let Some(draft) = self.form.as_mut().and_then(|f| f.agent.as_mut()) else {
            return Ok(());
        };
        let Some(profile) = draft.save(&target)? else {
            return Ok(());
        };
        for entry in self.sessions.iter_mut() {
            if entry.tab.world_id.as_deref() == Some(world_id.as_str()) || entry.tab.world == Some(index) {
                entry.tab.agent_profile_saved(&profile);
            }
        }
        Ok(())
    }

    /// The world editor's Scripts section for the session's saved world.
    fn edit_scripts(&mut self, id: SessionId) {
        let Some(index) = self.sessions.get(id).and_then(|e| e.tab.world) else {
            return;
        };
        if index >= self.settings.worlds.len() {
            return;
        }
        self.edit_world(Some(index));
        if let Some(form) = &mut self.form {
            form.section = crate::world_form::Section::Scripts;
        }
    }

    /// Open the world editor on a saved world (or a new one).
    fn edit_world(&mut self, index: Option<usize>) {
        let form = match index.and_then(|i| self.settings.worlds.get(i).cloned()) {
            Some(world) => {
                let library = self.library(&world.world_id.clone());
                WorldForm::edit(index, world).with_library(library)
            }
            None => WorldForm::new_world(),
        };
        #[allow(unused_mut)]
        let mut form = form.with_vault_name(self.vault.name());
        // Whether the saved password is really there, so the Login section can say when not.
        let wake = Arc::clone(&self.waker);
        form.check_saved_password(Arc::clone(&self.vault), move || wake());
        #[cfg(feature = "agent")]
        if let Some(services) = &self.agent {
            let world = index.and_then(|i| self.settings.worlds.get(i)).map(|w| {
                if worlds::valid_world_id(&w.world_id) {
                    wandur_core::agent::AgentWorld::Id(w.world_id.clone())
                } else {
                    wandur_core::agent::AgentWorld::endpoint(&w.host, w.port)
                }
            });
            form.agent = Some(crate::agent_settings::AgentDraft::open(
                Arc::clone(services),
                world.as_ref(),
                None,
            ));
        }
        self.form = Some(form);
    }

    /// Save world from the editor: the password through the vault (in the C# order, see
    /// `login::credentials`), then the world. A vault or settings failure keeps the editor open
    /// with the error. Returns the saved world's index.
    fn save_world_with_login(
        &mut self,
        index: Option<usize>,
        mut world: SavedWorld,
        password: &str,
        remember: bool,
    ) -> Result<usize, String> {
        // The editor's world by its index, checked against its id: should the list have changed
        // while the editor was open, the world is found by id rather than another one replaced.
        let index = index.and_then(|i| {
            let same = |w: &SavedWorld| w.world_id == world.world_id;
            if self.settings.worlds.get(i).is_some_and(same) || !worlds::valid_world_id(&world.world_id) {
                Some(i)
            } else {
                self.settings.worlds.iter().position(same)
            }
        });
        let original = index.and_then(|i| self.settings.worlds.get(i)).cloned();
        // The vault key binds the world id, so a new world gets its id first.
        if !worlds::valid_world_id(&world.world_id) {
            let db = self.db.as_ref();
            world.world_id = db
                .and_then(|db| {
                    db.read(|c| worlds::find_world(c, &world.host, world.port))
                        .ok()
                        .flatten()
                })
                .unwrap_or_else(worlds::new_world_id);
        }
        // A new password is only kept when the settings that refer to it are written, so the
        // settings are written here and now (a small file) rather than on the saver's thread.
        let persist_dir = self.saver.as_ref().and(self.data_dir.clone());
        let settings = &self.settings;
        // A saved password moved to a new key (the port or TLS changed) is written now too, so the
        // settings never point at the old entry once it is removed.
        let old_key = original.as_ref().and_then(|o| wandur_core::login::vault::key(o).ok());
        let persist = |saved: &SavedWorld| -> Result<(), String> {
            let Some(dir) = &persist_dir else { return Ok(()) };
            let moved = old_key.is_some() && wandur_core::login::vault::key(saved).ok() != old_key;
            if password.is_empty() && !moved {
                return Ok(());
            }
            let mut next = settings.clone();
            match index {
                Some(i) if i < next.worlds.len() => next.worlds[i] = saved.clone(),
                _ => next.worlds.push(saved.clone()),
            }
            next.save(dir).map_err(|e| e.to_string())
        };
        let saved = wandur_core::login::credentials::save_world(
            self.vault.as_ref(),
            original.as_ref(),
            world,
            password,
            remember,
            persist,
        )?;
        for notice in saved.notices {
            self.notes.insert(0, notice);
        }
        Ok(self.save_world(index, saved.world))
    }

    /// Write the world editor's macro changes for `world_id` (on the writer thread), keep the
    /// cache current and give open sessions of that world the saved macros at once.
    fn save_library(&mut self, world_id: &str, change: LibraryChange) {
        if !worlds::valid_world_id(world_id) {
            return;
        }
        if let Some(writer) = &self.db_writer {
            let id = world_id.to_string();
            let LibraryChange { before, after } = change.clone();
            writer.submit(Box::new(move |c| scripts::save_changes(c, &id, &before, &after)));
        }
        self.libraries.insert(world_id.to_string(), change.after.clone());
        let macros: Vec<_> = change.after.iter().filter_map(LibraryEntry::as_saved_macro).collect();
        let now = Instant::now();
        for entry in self.sessions.iter_mut() {
            if entry.tab.world_id.as_deref() == Some(world_id) {
                let id = entry.tab.world_id.clone();
                entry.tab.set_macros(id, macros.clone(), now);
                // A pack script's send choice reaches open sessions at once (C#
                // `SetAllowSendAsync` restarts the script); other script edits wait for Reload.
                for script in change.after.iter().filter(|e| e.is_pack()) {
                    entry
                        .tab
                        .set_script_send_policy(&script.id, script.restricted_send(), now);
                }
            }
        }
    }

    /// The scripts the directory supplies for this address (its listing's pack), installed or
    /// upgraded in the world's library before the session takes it (C# `ApplyPack` at open).
    fn apply_pack(&mut self, endpoint: &Endpoint, world: Option<usize>) {
        let Some(listing) = self
            .directory_status
            .catalog
            .as_ref()
            .and_then(|c| c.find_endpoint(&endpoint.host, endpoint.port, endpoint.tls))
        else {
            return;
        };
        let supplied = listing.supported_scripts();
        if supplied.is_empty() {
            return;
        }
        let Some(world_id) = self.session_world_id(endpoint, world) else {
            return;
        };
        let before = self.library(&world_id);
        if let Some(after) = wandur_core::scripting::packs::apply(&before, &world_id, &supplied) {
            self.save_library(&world_id, LibraryChange { before, after });
        }
    }

    /// Run one whole frame (logic and UI) outside eframe: headless scenes and tests.
    #[doc(hidden)]
    pub fn run_frame(&mut self, ctx: &egui::Context, input: egui::RawInput) -> egui::FullOutput {
        use eframe::App as _;
        let mut frame = eframe::Frame::_new_kittest();
        ctx.run_ui(input, |ui| {
            self.logic(ui.ctx(), &mut frame);
            self.ui(ui, &mut frame);
        })
    }

    /// What a scene waits for.
    #[doc(hidden)]
    pub fn scene_probe(&self) -> SceneProbe {
        let first = self.sessions.iter().next();
        let last = self.sessions.iter().last();
        let status = &self.directory_status;
        SceneProbe {
            csharp_import_looked: self.csharp_import.as_ref().is_some_and(|d| !d.busy()),
            catalog_worlds: status.catalog.as_ref().map_or(0, |c| c.len()),
            directory_failed: status.catalog.is_none() && !status.loading && status.warning.is_some(),
            art_pending: self.art.pending(),
            session_text: first.map(|e| e.tab.terminal.transcript()).unwrap_or_default(),
            map_rooms: first.map_or(0, |e| e.tab.map.tracker().room_count()),
            history: first.map_or(0, |e| e.tab.history_len()),
            last_text: last.map(|e| e.tab.terminal.transcript()).unwrap_or_default(),
            last_private: last.is_some_and(|e| e.tab.private_input()),
            last_input: last.map_or(0, |e| e.tab.input.chars().count()),
            last_ghost: last.and_then(|e| e.view.ghost.clone()),
            last_tail_rows: last.map_or(0, |e| e.view.tail_rows_drawn),
            last_pending_link: last.and_then(|e| e.view.pending_link.clone()),
            last_style_tip: last.is_some_and(|e| e.tab.style_tip_shown()),
            last_command_help: last.map(|e| e.view.command_help.shown.clone()).unwrap_or_default(),
            steps_done: self.scene_input.is_empty(),
            last_vitals: last.map_or(0, |e| e.view.vitals.len()),
            last_messages: last.map_or(0, |e| e.tab.protocol.messages.len()),
            last_server_details: last.is_some_and(|e| e.tab.protocol.messages.has_server_details()),
            history_ready: self.history_window.as_ref().is_some_and(HistoryWindow::ready),
            mark_open: self.mark.is_some(),
            scripts_running: last.map_or(0, |e| e.tab.scripts.entries.iter().filter(|s| s.is_running()).count()),
            scripts_menu: last.is_some_and(|e| e.view.scripts_menu),
            agent_menu: last.is_some_and(|e| e.view.agent_menu),
            script_completion: self.form.as_ref().is_some_and(|f| f.macros.editor.completion.is_some()),
            script_selected: self.form.as_ref().is_some_and(|f| {
                f.section == crate::world_form::Section::Scripts && f.macros.selected_script.is_some()
            }),
            rail_panels: last.map(|e| e.view.rail.shown.clone()).unwrap_or_default(),
            rail_widgets: last.map_or(0, |e| e.view.rail.drawn.len()),
            vitals_text: last
                .map(|e| {
                    e.view
                        .vitals
                        .iter()
                        .map(|c| format!("{} {}", c.label, c.values()))
                        .collect()
                })
                .unwrap_or_default(),
            map_scene_pending: first
                .and_then(|e| self.full_maps.get(&e.tab.id))
                .is_some_and(MapViewState::scene_pending)
                || self.map_editor_scene.is_some()
                || !self.map_seed.is_empty()
                || self.map_import_scene.is_some()
                || self.map_import.as_ref().is_some_and(|d| d.busy())
                || self.full_maps.values().any(MapViewState::scene_pending),
            inference_pending: first.map_or(0, |e| {
                e.tab
                    .map
                    .tracker()
                    .inference_candidates()
                    .filter(|r| r.inferred_key.is_none())
                    .count()
            }),
            strip_shown: self
                .active_session
                .and_then(|id| self.sessions.get(id))
                .is_some_and(|e| e.tab.strip.is_some()),
            update_shown: self.updates.visible(self.active_private()),
            tabs_ready: self.scene_tabs.as_ref().is_none_or(|scene| {
                let prompts = self
                    .sessions
                    .iter()
                    .filter(|e| e.tab.terminal.transcript().contains("Lantern Crossroads >"))
                    .count();
                prompts >= scene.lantern
                    && (scene.toast.is_none() || self.toast.is_some())
                    && self
                        .sessions
                        .iter()
                        .all(|e| e.tab.status != crate::session_tab::Status::Connecting)
            }),
        }
    }

    /// A session tabs scene: names the sessions and shows the chosen one at the start, keeps
    /// new output unseen only on the chosen background tabs, and makes its toast's delete.
    fn run_scene_tabs(&mut self) {
        let Some(scene) = self.scene_tabs.clone() else {
            return;
        };
        let ids: Vec<SessionId> = self.sessions.iter().map(|e| e.tab.id).collect();
        if !self.scene_tabs_started {
            self.scene_tabs_started = true;
            for (id, name) in ids.iter().zip(&scene.names) {
                if let Some(e) = self.sessions.get_mut(*id) {
                    e.tab.custom_name = Some(name.clone());
                }
            }
            if let Some(id) = ids.get(scene.active) {
                self.focus_session(*id);
            }
            if let Some(SceneToast::World(i)) = scene.toast {
                self.delete_world(i);
                self.scene_toast_done = true;
            }
        }
        for (n, id) in ids.iter().enumerate() {
            if n != scene.active
                && !scene.activity.contains(&n)
                && let Some(e) = self.sessions.get_mut(*id)
            {
                e.tab.mark_seen();
            }
        }
        if let Some(SceneToast::Rooms(count)) = scene.toast
            && !self.scene_toast_done
            && let Some(&id) = ids.get(scene.active)
            && let Some(entry) = self.sessions.get_mut(id)
        {
            let tracker = entry.tab.map.tracker_mut();
            if tracker.room_count() >= 21 {
                let current = tracker.current_id().map(str::to_string);
                let victims: Vec<String> = tracker
                    .rooms()
                    .map(|r| r.id.clone())
                    .filter(|r| Some(r) != current.as_ref())
                    .take(count)
                    .collect();
                let rooms = tracker.remove_rooms(&victims);
                if let Some(edit) = tracker.last_edit() {
                    self.actions.push(AppAction::MapDeleted(
                        id,
                        crate::map_view::editor::Deleted {
                            rooms,
                            edit,
                            ..Default::default()
                        },
                    ));
                }
                self.scene_toast_done = true;
            }
        }
    }

    /// The next scene typing step, when the last session's output asks for it.
    fn run_scene_input(&mut self, ctx: &egui::Context) {
        let Some(step) = self.scene_input.front_mut() else {
            return;
        };
        let Some(entry) = self.sessions.iter_mut().last() else {
            return;
        };
        let transcript = entry.tab.terminal.transcript();
        if !step.when.is_empty() {
            if !transcript.contains(&step.when) {
                return;
            }
            entry.tab.input = step.text.clone();
            entry.tab.caret_to_end = true;
            if step.submit {
                entry.tab.submit();
            } else {
                // As if typed: the command box offers what typing would.
                entry.view.command_help.treat_next_as_typed();
            }
            step.when.clear();
        }
        if let Some((when, after)) = &step.after {
            if !transcript.contains(when.as_str()) {
                return;
            }
            match after {
                SceneAfter::ScrollTo(text) => {
                    let term = &mut entry.tab.terminal;
                    let found = (1..=term.history_len()).find(|&offset| {
                        term.set_display_offset(offset);
                        crate::a11y::row_text(term, 0).contains(text.as_str())
                    });
                    if found.is_none() {
                        term.scroll_to_bottom();
                    }
                }
                SceneAfter::ClickLink => {
                    if let Some(at) = transcript.find("http") {
                        let url = transcript[at..].split_whitespace().next().unwrap_or_default();
                        let _ = crate::terminal_view::request_link(&mut entry.view, url);
                    }
                }
                SceneAfter::History => {
                    self.scene_input.pop_front();
                    self.open_history(ctx);
                    return;
                }
                SceneAfter::ScriptsMenu(name) => {
                    let id = entry
                        .tab
                        .scripts
                        .entries
                        .iter()
                        .find(|s| &s.name == name)
                        .map(|s| s.id.clone());
                    if let Some(id) = id {
                        entry.tab.switch_script(&id, true, Instant::now());
                    }
                    entry.view.scripts_menu = true;
                }
                SceneAfter::AgentMenu(decision) => {
                    #[cfg(feature = "agent")]
                    entry
                        .tab
                        .agent
                        .runner
                        .note_activity(decision, wandur_core::agent::AgentStatus::Paused);
                    #[cfg(not(feature = "agent"))]
                    let _ = decision;
                    entry.view.agent_menu = true;
                    entry.view.agent_activity_open = true;
                }
                SceneAfter::MarkChannel(start) => {
                    if let Some(line) = transcript.lines().find(|l| l.starts_with(start.as_str())) {
                        let id = entry.tab.id;
                        self.actions
                            .push(AppAction::MarkChannel(id, line.trim_end().to_string(), None));
                    }
                }
                SceneAfter::Diagnostics(tab, select) => {
                    entry.view.page = crate::terminal_view::Page::Diagnostics;
                    entry.view.diagnostics.tab = *tab;
                    if *tab == crate::diagnostics_view::DiagTab::Console {
                        entry.view.diagnostics.console_to_top = true;
                    }
                    if let Some(name) = select {
                        let messages = &mut entry.tab.protocol.messages;
                        let id = messages.entries().filter(|e| e.title() == name).map(|e| e.id).last();
                        if id.is_none() {
                            // Not here yet: try again next frame.
                            return;
                        }
                        messages.set_follow(false);
                        messages.select(id);
                    }
                }
            }
        }
        self.scene_input.pop_front();
    }

    /// Stop background work and write anything pending (what eframe's exit does).
    #[doc(hidden)]
    pub fn shutdown(&mut self) {
        use eframe::App as _;
        self.on_exit();
    }

    /// Open something at start (for screenshots and trying a view directly).
    fn show_at_start(&mut self, what: &str) {
        match what {
            "settings" => self.settings_dialog = Some(SettingsDialog::new(&self.settings)),
            "settings:general:history" => {
                let mut dialog = SettingsDialog::new(&self.settings);
                dialog.open_retention = true;
                self.settings_dialog = Some(dialog);
            }
            other if other.starts_with("settings:") => {
                let mut dialog = SettingsDialog::new(&self.settings);
                if let Some(section) = crate::settings_dialog::Section::from_name(&other["settings:".len()..]) {
                    dialog.section = section;
                }
                self.settings_dialog = Some(dialog);
            }
            "directory" => workspace::open_panel(&mut self.dock, Tab::Directory),
            other => {
                // An unpinned panel slid out, as if its strip tab was clicked.
                if let Some(tab) = other.strip_prefix("unpinned:").and_then(Tab::from_name) {
                    self.auto.show(tab, true);
                }
                if let Some(id) = other.strip_prefix("world:") {
                    workspace::open_panel(&mut self.dock, Tab::Directory);
                    self.dir_view.exploring = Some(id.to_string());
                }
            }
        }
    }

    /// Edit a session's map (the C# `OpenMapEditor`): its full map shown on the Map page with
    /// Edit on; starting to edit selects the room selected on the mini map (else the current
    /// room).
    fn open_map_editor(&mut self, id: SessionId) {
        let Some(entry) = self.sessions.get(id) else {
            return;
        };
        let tracker = entry.tab.map.tracker();
        let state = self.full_maps.entry(id).or_insert_with(MapViewState::full);
        if !state.is_editor() {
            let selected = self
                .maps
                .get(&id)
                .and_then(|m| m.selected.clone())
                .or_else(|| state.selected.clone())
                .or_else(|| tracker.current_id().map(str::to_string));
            state.set_editing(true);
            if let Some(room) = selected {
                state.start_on(tracker, &room);
            }
        }
        self.open_full_map(id);
    }

    /// Show a session's full map: its Map page (or Play and Map side by side, if that is on),
    /// the session's document focused.
    fn open_full_map(&mut self, id: SessionId) {
        let Some(entry) = self.sessions.get_mut(id) else {
            return;
        };
        if !entry.view.map_shown() {
            entry.view.show_map(true);
        }
        self.full_maps.entry(id).or_insert_with(MapViewState::full);
        workspace::focus_tab(&mut self.dock, Tab::Session(id));
        self.active_session = Some(id);
        self.directory_active = false;
    }

    /// Cmd+Shift+M (Ctrl+Shift+M elsewhere): the active session's Map page, or back to Play.
    fn toggle_session_map(&mut self) {
        let Some(id) = self.active_session.filter(|_| !self.directory_active) else {
            return;
        };
        let Some(entry) = self.sessions.get_mut(id) else {
            return;
        };
        if entry.view.page == crate::terminal_view::Page::Map && !entry.view.split {
            entry.view.show_map(false);
            entry.tab.focus_input = true;
        } else {
            self.open_full_map(id);
        }
    }

    /// Play and Map side by side for the active session, or back to the page shown.
    fn toggle_session_split(&mut self) {
        let Some(id) = self.active_session.filter(|_| !self.directory_active) else {
            return;
        };
        if let Some(entry) = self.sessions.get_mut(id) {
            let on = !(entry.view.split && entry.view.page != crate::terminal_view::Page::Diagnostics);
            entry.view.set_split(on);
            entry.tab.focus_input = true;
        }
    }

    /// Install a classifier package off the UI thread; sessions start classifying when it is
    /// ready.
    fn install_classifier(&mut self, path: PathBuf, ctx: &egui::Context) {
        let Some(service) = self.classification.clone() else {
            return;
        };
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("wandur-classifier-install".into())
            .spawn(move || {
                service.install_from(&path);
                ctx.request_repaint();
            });
    }

    /// A scene's map editor and seeded map, once the first session is far enough.
    fn run_map_scenes(&mut self) {
        let Some(entry) = self.sessions.iter_mut().next() else {
            return;
        };
        let id = entry.tab.id;
        if !self.map_seed.is_empty() && entry.tab.map.is_loaded() && entry.tab.map.tracker().room_count() > 0 {
            let mut tracker = wandur_core::map::RoomMapTracker::new();
            let seed = std::mem::take(&mut self.map_seed);
            for room in &seed {
                tracker.observe(room, None);
            }
            *entry.tab.map.tracker_mut() = tracker;
            if let Some(inference) = &mut entry.tab.inference {
                inference.rescan();
            }
        }
        if self.map_seed.is_empty()
            && self.map_editor_scene.is_some()
            && entry.tab.map.tracker().current_id().is_some()
            && entry.tab.map.tracker().room_count() >= 21
        {
            let scene = self.map_editor_scene.take().expect("checked");
            self.open_map_editor(id);
            if let Some(state) = self.full_maps.get_mut(&id) {
                state.set_scene(scene);
            }
        }
    }

    /// A map import scene, once the first session has a position: the Mudlet fixture read in
    /// File > Import map, or imported (the player put in its Road's End for the mini map).
    fn run_map_import_scene(&mut self, ctx: &egui::Context) {
        use crate::map_view::MapScene;
        let Some(scene) = self.map_import_scene else { return };
        let Some(entry) = self.sessions.iter().next() else {
            return;
        };
        let id = entry.tab.id;
        if !entry.tab.map.is_loaded() || entry.tab.map.tracker().current_id().is_none() {
            return;
        }
        self.map_import_scene = None;
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../wandur-core/tests/fixtures/mudlet/lantern-road-map.json");
        if scene == MapImportScene::Summary {
            self.open_map_import(ctx, Some(id));
            if let Some(dialog) = &mut self.map_import {
                dialog.read_file(&fixture);
            }
            return;
        }
        let Ok(prepared) = crate::map_import::read_path(&fixture, &|_| {}) else {
            return;
        };
        self.import_into_session(id, &prepared);
        self.toast = None;
        if scene != MapImportScene::Full {
            self.actions.retain(|a| !matches!(a, AppAction::OpenFullMap(_)));
        }
        let Some(entry) = self.sessions.get_mut(id) else {
            return;
        };
        match scene {
            MapImportScene::Mini => {
                entry.tab.map.tracker_mut().set_current_room("s:1001");
                // Zoomed out a little, so the area's labels are in view.
                self.maps
                    .entry(id)
                    .or_insert_with(MapViewState::mini)
                    .set_scene(MapScene::Zoom(0.45));
                if let Some(entry) = self.sessions.get_mut(id) {
                    entry.view.show_map(false);
                }
            }
            MapImportScene::Full => {
                entry.view.show_map(true);
                self.full_maps
                    .entry(id)
                    .or_insert_with(MapViewState::full)
                    .set_scene(MapScene::Area("mudlet:2".into()));
            }
            MapImportScene::EditLabel => {
                self.open_map_editor(id);
                if let Some(state) = self.full_maps.get_mut(&id) {
                    state.set_scene(MapScene::EditorLabel("mudlet:label:1:0".into()));
                }
            }
            MapImportScene::Summary => {}
        }
    }

    fn tab_options(&self, endpoint: Option<&Endpoint>, world: Option<usize>) -> TabOptions {
        let saved = world.and_then(|i| self.settings.worlds.get(i));
        let mapping = endpoint.and_then(|e| self.protocol_mapping(e, saved));
        TabOptions {
            name: saved.map(|w| w.name.clone()).unwrap_or_default(),
            world,
            scrollback: self.settings.scrollback,
            echo_commands: self.settings.local_echo,
            auto_reconnect: saved.map_or(self.settings.auto_reconnect, |w| w.auto_reconnect),
            charset: saved.map_or(wandur_core::Charset::Utf8, |w| w.charset),
            prompt_quiet: Duration::from_millis(self.settings.prompt_quiet_ms),
            character: saved.map(|w| w.username.trim().to_string()).unwrap_or_default(),
            password_prompt: saved.map_or_else(
                || wandur_core::login::DEFAULT_PASSWORD_PROMPT.to_string(),
                |w| w.password_prompt.clone(),
            ),
            login: saved.and_then(|w| self.login_config(w)),
            learn_words: self.settings.composer_suggestions,
            lua_scripts: self.settings.enable_lua_scripts,
            mapping,
            history: self.history_config(),
            channel_rules: saved.map(|w| w.channel_rules.clone()).unwrap_or_default(),
            world_theme: self.world_theme_for(endpoint, saved),
            codebase: self.world_codebase(endpoint, saved),
            command_style: saved.and_then(|w| w.command_style),
            global_command_style: self.settings.command_style,
            command_style_tip: !self.settings.command_style_tip_answered,
            maps: self.maps_config(endpoint, saved),
            inference: self.classification.as_ref().map(|service| {
                (
                    Arc::clone(service),
                    self.settings.classify_rooms_locally,
                    self.settings.room_classification_threshold,
                )
            }),
        }
    }

    /// Where a session's map is saved: under its saved world's id, else its address's world.
    fn maps_config(
        &self,
        endpoint: Option<&Endpoint>,
        saved: Option<&SavedWorld>,
    ) -> Option<crate::session_tab::MapsConfig> {
        let worker = self.map_worker.as_ref()?;
        let world = match saved.filter(|w| worlds::valid_world_id(&w.world_id)) {
            Some(w) => wandur_core::map::store::MapWorld::Id(w.world_id.clone()),
            None => {
                let e = endpoint?;
                wandur_core::map::store::MapWorld::Endpoint {
                    host: e.host.clone(),
                    port: e.port,
                }
            }
        };
        Some(crate::session_tab::MapsConfig {
            worker: Arc::clone(worker),
            world,
        })
    }

    /// The codebase that picks a session's channel family: the saved world's, else its directory
    /// listing's (by id, then by address). Empty when neither knows; then MSSP decides.
    fn world_codebase(&self, endpoint: Option<&Endpoint>, saved: Option<&SavedWorld>) -> String {
        if let Some(w) = saved.filter(|w| !w.codebase.trim().is_empty()) {
            return w.codebase.trim().to_string();
        }
        let catalog = self.directory_status.catalog.as_ref();
        let listing = saved
            .filter(|w| !w.listing_id.is_empty())
            .and_then(|w| catalog.and_then(|c| c.by_id(&w.listing_id)))
            .or_else(|| endpoint.and_then(|e| catalog.and_then(|c| c.find_endpoint(&e.host, e.port, e.tls))));
        listing
            .map(|l| l.features.codebase.trim().to_string())
            .unwrap_or_default()
    }

    /// A saved world changed: open sessions of it take its channel rules and codebase at once,
    /// keeping the line in flight (C# `RefreshChannelRules`), and its command style.
    fn refresh_channel_rules(&mut self) {
        type Update = (
            SessionId,
            Vec<wandur_core::channels::ChannelRule>,
            String,
            Option<CommandStyle>,
        );
        let updates: Vec<Update> = self
            .sessions
            .iter()
            .filter_map(|e| {
                let world = self.settings.worlds.get(e.tab.world?)?;
                let codebase = self.world_codebase(Some(&e.tab.endpoint), Some(world));
                Some((e.tab.id, world.channel_rules.clone(), codebase, world.command_style))
            })
            .collect();
        let (global, ask) = (self.settings.command_style, !self.settings.command_style_tip_answered);
        for (id, rules, codebase, style) in updates {
            if let Some(e) = self.sessions.get_mut(id) {
                e.tab.set_channel_rules(&rules, &codebase);
                e.tab.set_command_style(style, global, ask);
            }
        }
    }

    /// The one-time command style tip was answered: Use # makes TinTin++ the global style, Keep /
    /// leaves it; either way it never asks again, and the line waiting in the box then goes in
    /// the style now in force.
    fn answer_style_tip(&mut self, ctx: &egui::Context, id: SessionId, use_hash: bool) {
        if use_hash {
            self.settings.command_style = CommandStyle::TinTin;
        }
        self.settings.command_style_tip_answered = true;
        self.settings_changed(ctx);
        if let Some(e) = self.sessions.get_mut(id) {
            e.tab.submit();
            e.tab.focus_input = true;
        }
    }

    /// Open the Mark as channel dialog for a line of a session's transcript.
    fn open_mark_channel(&mut self, id: SessionId, example: &str, second: Option<&str>) {
        let Some(entry) = self.sessions.get(id) else { return };
        let tab = &entry.tab;
        let can_teach = !tab.is_demo() && tab.world.is_some_and(|i| i < self.settings.worlds.len());
        self.mark = Some(crate::mark_channel::MarkChannel::new(
            id,
            example,
            second,
            tab.recent_lines(crate::mark_channel::PREVIEW_LINES),
            std::sync::Arc::clone(tab.channels.rules()),
            can_teach,
        ));
    }

    /// Teach a session's world one channel rule: appended to the world's rules, saved like any
    /// world change, and in force for the next line (C# `TeachChannelRule`).
    pub fn teach_channel_rule(
        &mut self,
        id: SessionId,
        rule: wandur_core::channels::ChannelRule,
    ) -> Result<(), String> {
        let index = self
            .sessions
            .get(id)
            .filter(|e| !e.tab.is_demo())
            .and_then(|e| e.tab.world)
            .filter(|&i| i < self.settings.worlds.len())
            .ok_or_else(|| t(S::TeachChannelNoProfile).to_string())?;
        let mut world = self.settings.worlds[index].clone();
        world.channel_rules.push(rule);
        world.validate()?;
        let name = world.name.clone();
        self.save_world(Some(index), world);
        self.notes.insert(0, tf(S::TeachChannelSaved, &[&name]));
        Ok(())
    }

    /// Session history for new and open sessions, from the saved settings.
    fn history_config(&self) -> Option<HistoryConfig> {
        let store = self.history_store.as_ref()?;
        Some(HistoryConfig {
            store: Arc::clone(store),
            enabled: self.settings.history_enabled,
            retention_days: self.settings.history_retention_days,
            show_notice: !self.settings.hide_history_recording_notice,
            clock: Arc::clone(&self.history_clock),
        })
    }

    /// Remove expired history when the saved retention is new (at start and after Settings
    /// changed it), on a thread of its own. Recorders also prune hourly while they record.
    fn apply_history_retention(&mut self) {
        let days = self.settings.history_retention_days;
        if self.history_applied_days == Some(days) {
            return;
        }
        self.history_applied_days = Some(days);
        let Some(store) = self.history_store.clone() else {
            return;
        };
        let Some(before) = wandur_core::history::retention_cutoff(days, (self.history_clock)()) else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("wandur-history-prune".into())
            .spawn(move || {
                // A failure here is retried by the recorders' hourly pass.
                let _ = store.prune(before);
            });
        if let Ok(handle) = spawned {
            self.history_drains.push(handle);
        }
    }

    /// Open View > Session history (or bring it forward): open sessions write what they hold
    /// first, and the window's first read waits for that, so the UI never does.
    fn open_history(&mut self, ctx: &egui::Context) {
        if self.history_window.is_some() {
            ctx.send_viewport_cmd_to(
                egui::ViewportId::from_hash_of("session-history"),
                egui::ViewportCommand::Focus,
            );
            return;
        }
        let Some(store) = self.history_store.clone() else {
            return;
        };
        let flushes = history_flushes(&mut self.sessions);
        let repaint = ctx.clone();
        let mut model = HistoryModel::new(store, Arc::new(move || repaint.request_repaint()));
        model.wait_for(flushes);
        let mut window = HistoryWindow::new(model);
        if let Some(scene) = &self.history_scene {
            window.set_filters(&scene.query, &scene.world, &scene.character, scene.from, scene.until);
            window.open_first = Some(if scene.open_result {
                crate::history_view::BrowseTab::Results
            } else {
                crate::history_view::BrowseTab::Sessions
            });
        }
        self.history_window = Some(window);
    }

    /// Don't show again on the recording reminder: saved at once, so a failed write is known
    /// and the reminder stays on (the C# rule); then every session's reminder goes.
    fn hide_history_notice(&mut self) {
        let mut next = self.settings.clone();
        next.hide_history_recording_notice = true;
        let written = match (&self.saver, &self.data_dir) {
            (Some(_), Some(dir)) => next.save(dir).is_ok(),
            _ => true,
        };
        if !written {
            if let Some(e) = self.active_session.and_then(|id| self.sessions.get_mut(id)) {
                e.tab.strip = Some(StripNotice::Text(t(S::HistoryNoticePreferenceFailed).into()));
            }
            return;
        }
        self.settings = next;
        // The saver writes the same settings again, after anything it still has queued.
        self.save_settings();
        let config = self.history_config();
        for entry in self.sessions.iter_mut() {
            entry.tab.set_history(config.clone());
        }
    }

    /// The protocol mapping for a connection: the directory's current listing for this exact
    /// address first (it is the newer), else the one saved with the world.
    fn protocol_mapping(
        &self,
        endpoint: &Endpoint,
        saved: Option<&SavedWorld>,
    ) -> Option<wandur_core::protocol::mapping::WorldMapping> {
        let listed = self
            .directory_status
            .catalog
            .as_ref()
            .and_then(|c| c.find_endpoint(&endpoint.host, endpoint.port, endpoint.tls))
            .and_then(|l| l.mapping_for(&endpoint.host, endpoint.port, endpoint.tls))
            .cloned();
        listed.or_else(|| {
            saved
                .and_then(|w| w.protocol_mapping.clone())
                .filter(|m| m.is_for(endpoint))
        })
    }

    /// The directory reloaded: open sessions take their world's refreshed mapping.
    fn refresh_session_mappings(&mut self) {
        let Some(catalog) = self.directory_status.catalog.clone() else {
            return;
        };
        for entry in self.sessions.iter_mut() {
            let e = entry.tab.endpoint.clone();
            let listing = catalog.find_endpoint(&e.host, e.port, e.tls);
            if let Some(mapping) = listing.and_then(|l| l.mapping_for(&e.host, e.port, e.tls)) {
                entry.tab.set_mapping(mapping.clone());
            }
            // A world's theme follows its listing (the C# RefreshCatalogProfiles).
            if let Some(theme) = listing.and_then(|l| l.theme.clone()) {
                entry.tab.world_theme = Some(theme);
            }
        }
        // Saved worlds keep the theme their listing has now, so it applies offline too.
        let mut changed = false;
        for world in &mut self.settings.worlds {
            if let Some(theme) = catalog
                .find_endpoint(&world.host, world.port, world.tls)
                .and_then(|l| l.theme.as_ref())
                && world.theme.as_ref() != Some(theme)
            {
                world.theme = Some(theme.clone());
                changed = true;
            }
        }
        if changed {
            self.save_settings();
        }
    }

    /// A session's world theme: its listing's (by address), else its saved world's.
    fn world_theme_for(&self, endpoint: Option<&Endpoint>, saved: Option<&SavedWorld>) -> Option<WorldTheme> {
        let listed = endpoint.and_then(|e| {
            self.directory_status
                .catalog
                .as_ref()?
                .find_endpoint(&e.host, e.port, e.tls)?
                .theme
                .clone()
        });
        listed.or_else(|| saved.and_then(|w| w.theme.clone()))
    }

    /// What auto-login needs for a saved world, when it is turned on and complete.
    fn login_config(&self, world: &SavedWorld) -> Option<LoginConfig> {
        let username = world.username.trim();
        if !world.auto_login || username.is_empty() {
            return None;
        }
        let key = wandur_core::login::vault::key(world).ok()?;
        Some(LoginConfig {
            username: username.to_string(),
            key,
            username_prompt: world.username_prompt.clone(),
            password_prompt: world.password_prompt.clone(),
            vault: Arc::clone(&self.vault),
        })
    }

    fn open(&mut self, endpoint: Endpoint, world: Option<usize>) {
        let options = self.tab_options(Some(&endpoint), world);
        self.apply_pack(&endpoint, world);
        let id = self.sessions.open(endpoint, &options, Arc::clone(&self.waker));
        self.attach_macros(id);
        self.attach_agent(id);
        workspace::add_document(&mut self.dock, Tab::Session(id));
        self.active_session = Some(id);
        self.directory_active = false;
        if let Some(w) = world.and_then(|i| self.settings.worlds.get_mut(i)) {
            let now = unix_now();
            w.note_connected(now);
            let id = w.world_id.clone();
            if let Some(writer) = &self.db_writer
                && worlds::valid_world_id(&id)
            {
                writer.submit(Box::new(move |c| worlds::record_connection(c, &id, now)));
            }
            self.save_settings();
        }
    }

    /// Save the settings (on the saver's thread) and register any new or edited world address
    /// with the database (on the writer's thread).
    fn save_settings(&mut self) {
        self.assign_world_ids();
        if let Some(saver) = &self.saver {
            saver.save(&self.settings);
        }
        self.register_worlds();
    }

    /// Give new saved worlds their stable id: the one the database has for the address, else a
    /// new one. A short read on this thread, only when a world has no id yet.
    fn assign_world_ids(&mut self) -> bool {
        let db = self.db.as_ref();
        self.settings
            .assign_world_ids(|w| db.and_then(|db| db.read(|c| worlds::find_world(c, &w.host, w.port)).ok().flatten()))
    }

    /// Tell the writer about saved world addresses it has not seen: each address is added to its
    /// world, so an edited address keeps the world's id and the old address stays an alias.
    fn register_worlds(&mut self) {
        let current: Vec<(String, String, u16)> = self
            .settings
            .worlds
            .iter()
            .map(|w| (w.world_id.clone(), w.host.clone(), w.port))
            .collect();
        if current == self.worlds_registered {
            return;
        }
        let Some(writer) = &self.db_writer else { return };
        let new: Vec<(String, String, u16)> = current
            .iter()
            .filter(|w| !self.worlds_registered.contains(w))
            .cloned()
            .collect();
        writer.submit(Box::new(move |c| {
            for (id, host, port) in &new {
                match worlds::resolve_world(c, host, *port, Some(id)) {
                    Ok(_) => {}
                    // Two saved worlds at one address: the first keeps it (the C# rule).
                    Err(e @ wandur_core::db::DbError::WorldConflict { .. }) => eprintln!("wandur: {e}"),
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }));
        self.worlds_registered = current;
    }

    /// Store what the world form saved: an edit replaces the world (keeping its id), a new one
    /// is added and selected.
    fn save_world(&mut self, index: Option<usize>, world: SavedWorld) -> usize {
        let (index, added) = match index {
            Some(i) if i < self.settings.worlds.len() => {
                self.settings.worlds[i] = world;
                (i, false)
            }
            _ => {
                self.settings.worlds.push(world);
                self.panel.selected_world = Some(self.settings.worlds.len() - 1);
                (self.settings.worlds.len() - 1, true)
            }
        };
        self.save_settings();
        if added {
            self.adopt_sessions(index);
        }
        self.refresh_channel_rules();
        index
    }

    /// A world was just saved: sessions opened by address at its address (not from a saved
    /// world, not the demo) take it as their world now, so teaching a channel, Edit scripts and
    /// Configure agent work without reconnecting. Their macros follow its id when that differs.
    /// The map and the agent profile are kept by address, which `wandur.db` resolves to the same
    /// world, so they stay as they are (a running agent is not stopped). The caller refreshes
    /// the channel rules. Call after `save_settings`, which gives the world its id.
    fn adopt_sessions(&mut self, index: usize) {
        let Some(world) = self.settings.worlds.get(index) else {
            return;
        };
        let adopted: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|e| e.tab.world.is_none() && !e.tab.is_demo() && world.is_at(&e.tab.endpoint))
            .map(|e| e.tab.id)
            .collect();
        for id in adopted {
            let Some(entry) = self.sessions.get_mut(id) else {
                continue;
            };
            entry.tab.world = Some(index);
            let endpoint = entry.tab.endpoint.clone();
            let current = entry.tab.world_id.clone();
            if self.session_world_id(&endpoint, Some(index)) != current {
                self.attach_macros(id);
            }
        }
    }

    /// Where something a scene points at was drawn last frame: a panel's header, a saved world's
    /// row, a panel's whole leaf.
    pub fn scene_target(&self, target: crate::scene::Target) -> Option<egui::Rect> {
        use crate::scene::Target;
        let leaf = |tab: Tab| {
            let path = self.dock.find_tab(&tab)?;
            self.dock.leaf(path.node_path()).ok().map(|l| l.rect)
        };
        match target {
            Target::Header(tab) => leaf(tab)
                .map(|r| egui::Rect::from_min_size(r.min, egui::vec2(r.width(), self.chrome.skin.dock_header_height))),
            Target::Leaf(tab) => leaf(tab),
            Target::Session => {
                let path = self.dock.find_tab_from(|t| matches!(t, Tab::Session(_)))?;
                self.dock.leaf(path.node_path()).ok().map(|l| l.rect)
            }
            Target::SavedRow(i) => self.panel.row_rects.iter().find(|(w, _)| *w == i).map(|(_, r)| *r),
            Target::Dock => self.dock_rect.is_positive().then_some(self.dock_rect),
            Target::Picker => self.picker_rects.0.is_positive().then_some(self.picker_rects.0),
            Target::PickerField => self.picker_rects.1.is_positive().then_some(self.picker_rects.1),
        }
    }

    /// Windows and Linux: Alt alone (pressed and released with nothing else), Alt+F or F10 open
    /// the menu button's dropdown with the keyboard on its first item, as Chrome does.
    fn menu_keys(&mut self, ctx: &egui::Context) {
        if self.platform == Platform::Mac {
            return;
        }
        let (alt, other) = ctx.input(|i| {
            let other = i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_))
                    || matches!(e, egui::Event::PointerButton { pressed: true, .. })
            });
            (i.modifiers.alt, other)
        });
        let mut open = false;
        match (alt, self.alt_tap) {
            (true, None) => self.alt_tap = Some(!other),
            (true, Some(_)) if other => self.alt_tap = Some(false),
            (false, Some(clean)) => {
                open = clean;
                self.alt_tap = None;
            }
            _ => {}
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::F))
            || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F10))
        {
            self.alt_tap = alt.then_some(false);
            open = true;
        }
        if open && !self.menu_button.open {
            let entries = menus::button_entries(&menus::menus(self.platform, &self.menu_state()));
            self.menu_button.open_with_keyboard(&entries);
            ctx.request_repaint();
        } else if open {
            self.menu_button.close();
        }
    }

    /// The title bar's menu button (three lines): opens and closes the dropdown, which hangs
    /// from it (from its right edge on macOS, where it is the last title control).
    fn draw_menu_button(&mut self, ui: &mut Ui, rect: egui::Rect) {
        let id = ui.id().with("title-menu-button");
        let response = ui.interact(rect, id, egui::Sense::click());
        let open = self.menu_button.open;
        let hover = ui
            .ctx()
            .animate_bool_with_time(id.with("hover"), response.hovered() || open, 0.1);
        if hover > 0.0 {
            let fill = crate::theme::mix(self.chrome.toolbar.middle(), self.theme.text, 0.10);
            ui.painter().rect_filled(rect, 4.0, fill.gamma_multiply(hover));
        }
        widgets::paint_icon(ui, Icon::Menu, rect, self.chrome.icon);
        let label = t(S::MenuButton);
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, open, label));
        let response = response.on_hover_text(label);
        if response.clicked() {
            self.menu_button.toggle();
        }
        self.menu_button.button = Some(rect);
        self.menu_button.align_right = self.platform == Platform::Mac;
    }

    /// How this skin draws panel headers (the C# flat headers in System and Fleet, the shaded
    /// metal in Armored).
    fn header_look(&self) -> panel_header::HeaderLook {
        let theme = &self.theme;
        let chrome = &self.chrome;
        let muted_glyphs = crate::theme::mix(theme.text, theme.panel, 0.25);
        match chrome.skin.id {
            SkinId::System => panel_header::HeaderLook {
                height: chrome.skin.dock_header_height,
                fill: chrome.header.clone(),
                line: Some(theme.border),
                top_line: None,
                grip_x: 7.0,
                title_x: 23.0,
                title_size: 13.0,
                title_color: theme.text,
                grip_color: theme.muted,
                glyph: theme.muted,
                glyph_hover: muted_glyphs,
                buttons_on_hover: true,
            },
            SkinId::Fleet => panel_header::HeaderLook {
                height: chrome.skin.dock_header_height,
                fill: chrome.header.clone(),
                line: Some(chrome.rim_edge),
                top_line: Some(chrome.rim_edge),
                grip_x: 7.0,
                title_x: 23.0,
                title_size: 14.0,
                title_color: chrome.header_text,
                grip_color: muted_glyphs,
                glyph: chrome.icon,
                glyph_hover: chrome.icon,
                buttons_on_hover: true,
            },
            SkinId::Armored => panel_header::HeaderLook {
                height: chrome.skin.dock_header_height,
                fill: chrome.header.clone(),
                line: None,
                top_line: None,
                grip_x: 22.0,
                title_x: 38.0,
                title_size: 12.5,
                title_color: chrome.header_text,
                grip_color: muted_glyphs,
                glyph: chrome.icon,
                glyph_hover: chrome.icon,
                buttons_on_hover: false,
            },
        }
    }

    /// A tool panel's actions for its header.
    fn header_actions(&self, tab: Tab) -> Vec<panel_header::HeaderAction> {
        match tab {
            Tab::SavedWorlds => crate::saved_worlds_panel::header_actions(&self.panel),
            Tab::Map => {
                let walking = self
                    .active_session
                    .and_then(|id| self.sessions.get(id))
                    .is_some_and(|e| e.tab.map.is_walking());
                let id = self.active_session.unwrap_or(0);
                match self.maps.get(&id) {
                    Some(state) => crate::map_view::header_actions(state, self.settings.map_auto_center, walking),
                    None => crate::map_view::header_actions(
                        &crate::map_view::MapViewState::mini(),
                        self.settings.map_auto_center,
                        walking,
                    ),
                }
            }
            _ => Vec::new(),
        }
    }

    /// Which tool panels get a header this frame (alone in their leaf of the main dock), and
    /// whether their actions fit in it or drop to a row (from last frame's widths).
    fn plan_headers(&mut self, ui: &Ui, look: &panel_header::HeaderLook) {
        let mut heads = HashMap::new();
        for (path, leaf) in self.dock.iter_leaves() {
            if !path.surface.is_main() || leaf.tabs.len() != 1 || leaf.tabs[0].is_document() {
                continue;
            }
            let tab = leaf.tabs[0];
            let width = if leaf.rect.is_positive() {
                leaf.rect.width()
            } else {
                self.heads.get(&tab).map_or(240.0, |h| h.width)
            };
            let actions = self.header_actions(tab);
            let title = tab.label();
            let fits = panel_header::actions_fit(
                width,
                look.title_x,
                panel_header::title_minimum(ui, title, look.title_size),
                actions.len(),
                !look.buttons_on_hover,
            );
            let place = if fits {
                panel_header::ActionsPlace::Header
            } else {
                panel_header::ActionsPlace::Row(look.clone())
            };
            heads.insert(tab, shell::Head { width, place });
        }
        self.heads = heads;
    }

    /// Paint each planned header over its panel's (invisible) dock tab and run what was clicked.
    fn draw_headers(&mut self, ui: &mut Ui, look: &panel_header::HeaderLook) {
        let mut heads: Vec<(Tab, bool)> = self
            .heads
            .iter()
            .map(|(t, h)| (*t, matches!(h.place, panel_header::ActionsPlace::Row(_))))
            .collect();
        heads.sort_by_key(|(t, _)| t.kind_index());
        let mut cursor_over_header = None;
        for (tab, in_row) in heads {
            let Some(path) = self.dock.find_tab(&tab) else { continue };
            let Ok(leaf) = self.dock.leaf(path.node_path()) else {
                continue;
            };
            if !leaf.rect.is_positive() || leaf.tabs.len() != 1 {
                continue;
            }
            let leaf_rect = leaf.rect;
            let rect = egui::Rect::from_min_size(leaf_rect.min, egui::vec2(leaf_rect.width(), look.height));
            let actions = self.header_actions(tab);
            let out = panel_header::header(
                ui,
                egui::Id::new(("panel-header", tab)),
                rect,
                tab.label(),
                &actions,
                in_row,
                crate::autohide::can_hide(tab),
                look,
                &self.theme,
            );
            if ui.rect_contains_pointer(rect) {
                cursor_over_header = Some(out.on_grip);
            }
            if let Some(id) = out.action {
                match tab {
                    Tab::SavedWorlds => crate::saved_worlds_panel::on_action(id, &mut self.panel, &mut self.actions),
                    Tab::Map => {
                        self.maps
                            .entry(self.active_session.unwrap_or(0))
                            .or_insert_with(MapViewState::mini)
                            .header_click = Some(id);
                    }
                    _ => {}
                }
                ui.ctx().request_repaint();
            }
            match out.command {
                Some(panel_header::DockCommand::Float) => {
                    let at = leaf_rect.translate(egui::vec2(24.0, 24.0));
                    self.dock.detach_tab(path, at);
                }
                Some(panel_header::DockCommand::AutoHide) => self.actions.push(AppAction::Unpin(tab)),
                Some(panel_header::DockCommand::Close) => {
                    self.dock.remove_tab(path);
                    self.save_layout(true);
                }
                None => {}
            }
        }
        // The whole header drags, but only the grip shows the move cursor (as C#).
        if let Some(on_grip) = cursor_over_header
            && ui.ctx().dragged_id().is_none()
        {
            ui.ctx().set_cursor_icon(if on_grip {
                egui::CursorIcon::Move
            } else {
                egui::CursorIcon::Default
            });
        }
    }

    /// Show a session's tab and give its command box the keyboard.
    fn focus_session(&mut self, id: SessionId) {
        workspace::focus_tab(&mut self.dock, Tab::Session(id));
        self.active_session = Some(id);
        self.directory_active = false;
        if let Some(e) = self.sessions.get_mut(id) {
            e.tab.focus_input = true;
        }
    }

    /// Saved worlds' Connect (a double-click, Enter): back to the world's open session when it
    /// has one, so the same character is not logged in twice by accident; else a new session.
    fn go_to_world(&mut self, i: usize) {
        if i >= self.settings.worlds.len() {
            return;
        }
        self.panel.selected_world = Some(i);
        let open = self
            .sessions
            .iter()
            .find(|e| e.tab.world == Some(i) && !e.tab.is_closed())
            .map(|e| e.tab.id);
        match open {
            Some(id) => self.focus_session(id),
            None => {
                let endpoint = self.settings.worlds[i].endpoint();
                self.open(endpoint, Some(i));
            }
        }
    }

    /// A copy of a saved world under a new name, after it, with its settings and automation but
    /// not its saved password (a copy is usually for another character), selected (C#
    /// `DuplicateProfile`).
    fn duplicate_world(&mut self, i: usize) {
        let Some(original) = self.settings.worlds.get(i) else {
            return;
        };
        let mut name = tf(S::ScriptDuplicateName, &[&original.name]);
        // A name is at most 100 characters; a long one gives up its end to the copy marker.
        let over = name.chars().count().saturating_sub(100);
        if over > 0 {
            let keep = original.name.chars().count().saturating_sub(over).max(1);
            let short: String = original.name.chars().take(keep).collect();
            name = tf(S::ScriptDuplicateName, &[&short]);
        }
        let copy = SavedWorld {
            name,
            password_id: None,
            auto_login: false,
            last_connected: None,
            connections: 0,
            ..original.clone()
        };
        self.settings.worlds.insert(i + 1, copy);
        for e in self.sessions.iter_mut() {
            if let Some(w) = e.tab.world.as_mut()
                && *w > i
            {
                *w += 1;
            }
        }
        self.panel.selected_world = Some(i + 1);
        self.save_settings();
    }

    /// Show a saved world's page in Find a MUD.
    fn explore_world(&mut self, i: usize) {
        let (Some(world), Some(catalog)) = (self.settings.worlds.get(i), self.directory_status.catalog.clone()) else {
            return;
        };
        let Some(listing) = catalog
            .by_id(&world.listing_id)
            .or_else(|| catalog.find_endpoint(&world.host, world.port, world.tls))
        else {
            return;
        };
        let id = listing.id.clone();
        self.dir_view.explore(&catalog, id);
        workspace::open_panel(&mut self.dock, Tab::Directory);
        workspace::focus_tab(&mut self.dock, Tab::Directory);
        self.directory_active = true;
    }

    /// Remove a saved world (sessions opened from it keep running, unattached).
    /// Delete saved world `i`. The undo toast offers it back exactly as it was; its saved
    /// password is forgotten only when the toast goes without Undo ([`Self::finish_toast`]).
    fn delete_world(&mut self, i: usize) {
        if i < self.settings.worlds.len() {
            let removed = self.settings.worlds.remove(i);
            let mut linked = Vec::new();
            for e in self.sessions.iter_mut() {
                e.tab.world = match e.tab.world {
                    Some(w) if w == i => {
                        linked.push(e.tab.id);
                        None
                    }
                    Some(w) if w > i => Some(w - 1),
                    other => other,
                };
            }
            self.panel.selected_world = self.panel.selected_world.filter(|&s| s < self.settings.worlds.len());
            self.save_settings();
            let text = tf(S::ToastDeletedWorld, &[&removed.name]);
            self.set_toast(Toast::new(
                text,
                Undo::World {
                    index: i,
                    world: Box::new(removed),
                    sessions: linked,
                },
            ));
        }
    }

    /// Put a deleted saved world back at its place, exactly as it was (its password reference
    /// too: the saved password was never forgotten), with its sessions pointing at it again.
    fn restore_world(&mut self, index: usize, world: SavedWorld, sessions: &[SessionId]) {
        let index = index.min(self.settings.worlds.len());
        self.settings.worlds.insert(index, world);
        for e in self.sessions.iter_mut() {
            e.tab.world = match e.tab.world {
                Some(w) if w >= index => Some(w + 1),
                None if sessions.contains(&e.tab.id) => Some(index),
                other => other,
            };
        }
        self.panel.selected_world = Some(index);
        self.save_settings();
    }

    /// Show a new undo toast. The one shown before (here or in the world editor) goes: its
    /// delete is no longer undoable from a toast.
    pub fn set_toast(&mut self, toast: Toast) {
        if let Some(old) = self.toast.take() {
            self.finish_toast(old);
        }
        if let Some(form) = &mut self.form {
            form.toast = None;
        }
        self.toast = Some(toast);
    }

    /// A toast went without Undo: what it kept for Undo goes for good. For a saved world that is
    /// its saved password, unless a saved world still refers to it (after the settings, as C#).
    fn finish_toast(&mut self, toast: Toast) {
        if let Undo::DetachedMap { key, .. } = toast.undo {
            self.detached_maps.retain(|(k, _)| *k != key);
            return;
        }
        if let Undo::World { world, .. } = toast.undo
            && world.password_id.is_some()
            && !self.settings.worlds.iter().any(|w| w.password_id == world.password_id)
            && let Some(notice) = wandur_core::login::credentials::forget(self.vault.as_ref(), &world)
        {
            self.notes.insert(0, notice);
        }
    }

    /// The toast's Undo (if the toast with this number is still shown).
    fn undo_toast(&mut self, serial: u64) {
        let Some(toast) = self.toast.take_if(|t| t.serial == serial) else {
            return;
        };
        match toast.undo {
            Undo::Map { session, edit } => {
                let Some(entry) = self.sessions.get_mut(session) else {
                    return;
                };
                if entry.tab.map.tracker().last_edit() != Some(edit) {
                    return;
                }
                match self.full_maps.get_mut(&session).filter(|m| m.editor.is_some()) {
                    Some(state) => {
                        let outcome = crate::map_view::editor::apply(
                            state,
                            entry.tab.map.tracker_mut(),
                            crate::map_view::editor::EditAction::Undo,
                        );
                        if outcome.rescan
                            && let Some(inference) = &mut entry.tab.inference
                        {
                            inference.rescan();
                        }
                    }
                    None => {
                        entry.tab.map.tracker_mut().undo();
                    }
                }
            }
            Undo::World { index, world, sessions } => self.restore_world(index, *world, &sessions),
            Undo::DetachedMap { key, edit } => {
                let Some(i) = self.detached_maps.iter().position(|(k, _)| *k == key) else {
                    return;
                };
                let (_, mut done) = self.detached_maps.remove(i);
                if done.tracker.last_edit() == Some(edit)
                    && done.tracker.undo()
                    && let Some(maps) = &self.map_worker
                {
                    let _ = maps.save(done.world.clone(), done.tracker.snapshot());
                }
            }
            Undo::Script { .. } | Undo::Macro { .. } => {}
        }
    }

    /// Draw the main window's toast over the dock, and let it go when its time is up or its
    /// delete is no longer the map's next undo step.
    fn show_toast(&mut self, ctx: &egui::Context) {
        if let Some(Toast {
            undo: Undo::Map { session, edit },
            ..
        }) = &self.toast
        {
            let current = self
                .sessions
                .get(*session)
                .and_then(|e| e.tab.map.tracker().last_edit());
            if current != Some(*edit)
                && let Some(old) = self.toast.take()
            {
                self.finish_toast(old);
            }
        }
        // The world editor's toast is newer: it replaces this one.
        if let (Some(main), Some(form)) = (&self.toast, &self.form)
            && form.toast.as_ref().is_some_and(|t| t.serial > main.serial)
            && let Some(old) = self.toast.take()
        {
            self.finish_toast(old);
        }
        let Some(toast) = &mut self.toast else {
            return;
        };
        let (out, expired) = crate::toast::show(
            ctx,
            egui::Id::new("undo-toast"),
            self.dock_rect,
            crate::toast::MAIN_LIFT,
            toast,
            &self.theme,
            Instant::now(),
        );
        if expired {
            if let Some(old) = self.toast.take() {
                self.finish_toast(old);
            }
        } else if out == crate::toast::Output::Undo {
            let serial = toast.serial;
            self.actions.push(AppAction::UndoToast(serial));
        }
    }

    /// Save what the settings dialog saved. It does not edit the saved worlds, so they stay as
    /// they are now: a world saved (a new password reference) or connected to while it was open
    /// is not put back as it was.
    fn save_preferences_from_dialog(&mut self, ctx: &egui::Context, settings: Settings) {
        let settings = Settings {
            worlds: self.settings.worlds.clone(),
            ..settings
        };
        self.save_preferences(ctx, settings);
    }

    /// Save preferences from the settings dialog: apply them, then show or close the Channels
    /// panel if that choice changed.
    fn save_preferences(&mut self, ctx: &egui::Context, settings: Settings) {
        let channels_before = self.settings.show_channels;
        // What the dialog does not edit, and may have changed while it was open (an update
        // check finished), stays as it is now.
        let settings = Settings {
            install_id: self.settings.install_id.clone(),
            last_update_check: self.settings.last_update_check.clone(),
            skipped_update_version: self.settings.skipped_update_version.clone(),
            command_style_tip_answered: self.settings.command_style_tip_answered,
            ..settings
        };
        self.settings = settings;
        crate::settings_dialog::apply_language(&self.settings.language);
        if self.settings.show_channels != channels_before
            && self.settings.show_channels != self.panel_visible(Tab::Channels)
        {
            self.toggle_panel(Tab::Channels);
        }
        self.settings_changed(ctx);
    }

    /// Apply changed settings to what is open, then save.
    fn settings_changed(&mut self, ctx: &egui::Context) {
        self.settings.clamp(wandur_term::MAX_SCROLLBACK);
        self.fonts = TermFonts::new(self.settings.font_size);
        self.pacer.set_fps(self.settings.output_fps);
        for entry in self.sessions.iter_mut() {
            let tab = &mut entry.tab;
            if tab.terminal.scrollback() != self.settings.scrollback {
                tab.terminal.set_scrollback(self.settings.scrollback);
            }
            tab.echo_commands = self.settings.local_echo;
            tab.set_learning(self.settings.composer_suggestions);
            tab.set_lua_scripts(self.settings.enable_lua_scripts, Instant::now());
            let world = tab.world.and_then(|i| self.settings.worlds.get(i));
            tab.set_command_style(
                world.and_then(|w| w.command_style),
                self.settings.command_style,
                !self.settings.command_style_tip_answered,
            );
            if let Some(inference) = &mut tab.inference {
                inference.configure(
                    self.settings.classify_rooms_locally,
                    self.settings.room_classification_threshold,
                );
            }
        }
        // Only saved settings reach history (never a preview in the open dialog).
        let history = self.history_config();
        for entry in self.sessions.iter_mut() {
            entry.tab.set_history(history.clone());
        }
        self.apply_history_retention();
        self.refresh_appearance(ctx);
        self.install.apply(&self.settings);
        if self.settings.directory_url != self.directory_setting_seen {
            self.directory_setting_seen = self.settings.directory_url.clone();
            self.directory = start_directory(
                &self.directory_setting_seen,
                self.data_dir.as_deref(),
                &self.fetcher,
                ctx,
            );
            self.install.set_base(self.directory.base());
            self.updates.replace_service(update_service(
                self.update_source.as_ref(),
                self.update_version.as_deref(),
                self.update_clock.as_ref(),
                self.directory.base(),
                &self.install,
            ));
            self.requested_fetch = false;
            self.art.forget_failures();
        }
        self.save_settings();
    }

    /// Add a directory listing to the saved worlds (the C# `SaveSelectedWorld`). Returns the
    /// saved world's index.
    fn save_listing(&mut self, id: &str, tls: bool) -> Option<usize> {
        let catalog = self.directory_status.catalog.clone()?;
        let Some(listing) = catalog.by_id(id) else {
            self.dir_view.feedback = t(S::WorldNoLongerInDirectory).into();
            return None;
        };
        if !listing.can_connect() {
            self.dir_view.feedback = t(S::ListingHasNoMudConnection).into();
            return None;
        }
        let port = if tls {
            listing.tls_port
        } else {
            listing.port.or(listing.tls_port)
        };
        let Some(port) = port else {
            self.dir_view.feedback = t(S::WorldHasNoTlsPort).into();
            return None;
        };
        let tls = tls || listing.port.is_none();
        let world = SavedWorld {
            name: listing.name.chars().take(100).collect(),
            host: listing.host.clone(),
            port,
            tls,
            listing_id: listing.id.clone(),
            protocol_mapping: listing.mapping_for(&listing.host, port, tls).cloned(),
            codebase: listing.features.codebase.trim().chars().take(100).collect(),
            theme: listing.theme.clone(),
            ..SavedWorld::default()
        };
        if let Some(i) = self.settings.worlds.iter().position(|w| w.is_at(&world.endpoint())) {
            let saved = &mut self.settings.worlds[i];
            if saved.listing_id.is_empty() || saved.codebase.is_empty() {
                if saved.listing_id.is_empty() {
                    saved.listing_id = listing.id.clone();
                }
                if saved.codebase.is_empty() {
                    saved.codebase = world.codebase.clone();
                }
                self.save_settings();
            }
            self.dir_view.feedback = t(S::AlreadyInYourWorlds).into();
            return Some(i);
        }
        self.dir_view.feedback = tf(S::AddedToYourWorlds, &[&world.name]);
        self.settings.worlds.push(world);
        self.save_settings();
        Some(self.settings.worlds.len() - 1)
    }

    fn reset_layout(&mut self) {
        self.apply_preset(workspace::NEW_INSTALL);
    }

    /// Replace the layout with a preset (View > Layout), keeping the open sessions in the
    /// document area. Saved at once, so it is the layout the next start has; Restore Panels or
    /// another preset changes it again.
    fn apply_preset(&mut self, preset: workspace::Preset) {
        let (dock, auto) = workspace::preset_layout(preset);
        self.dock = dock;
        self.auto = auto;
        // In the order of the session tabs (the sessions follow it).
        let ids: Vec<SessionId> = self.sessions.iter().map(|e| e.tab.id).collect();
        for id in ids {
            workspace::add_document(&mut self.dock, Tab::Session(id));
        }
        if let Some(id) = self.active_session {
            workspace::focus_tab(&mut self.dock, Tab::Session(id));
        }
        self.save_layout(true);
    }

    /// Save the layout if it changed (`force`: now, whatever the clock).
    fn save_layout(&mut self, force: bool) {
        if !force && self.layout_checked.elapsed() < LAYOUT_CHECK {
            return;
        }
        self.layout_checked = Instant::now();
        let json = layout::to_json_with_split(&self.dock, &self.auto, chosen_split(self.split_share));
        if json != self.saved_layout
            && let (Some(saver), Some(text)) = (&self.layout_saver, &json)
        {
            saver.save(text.clone().into_bytes());
            self.saved_layout = json;
        }
    }

    fn apply(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                AppAction::Connect(endpoint) => {
                    self.settings.note_recent(&endpoint.to_string());
                    let world = self.settings.worlds.iter().position(|w| w.is_at(&endpoint));
                    self.save_settings();
                    self.open(endpoint, world);
                }
                AppAction::ConnectWorld(i) => {
                    if let Some(world) = self.settings.worlds.get(i) {
                        let endpoint = world.endpoint();
                        self.open(endpoint, Some(i));
                    }
                }
                AppAction::GoToWorld(i) => self.go_to_world(i),
                AppAction::DuplicateWorld(i) => self.duplicate_world(i),
                AppAction::ExploreWorld(i) => self.explore_world(i),
                AppAction::SaveWorld(id) => {
                    if let Some(e) = self.sessions.get(id) {
                        let endpoint = e.tab.endpoint.clone();
                        if !self.settings.worlds.iter().any(|w| w.is_at(&endpoint)) {
                            let mut world = SavedWorld::from_endpoint(endpoint.host.clone(), &endpoint);
                            world.auto_reconnect = e.tab.auto_reconnect();
                            if let Some(listing) = self
                                .directory_status
                                .catalog
                                .as_ref()
                                .and_then(|c| c.find_endpoint(&endpoint.host, endpoint.port, endpoint.tls))
                            {
                                world.name = listing.name.chars().take(100).collect();
                                world.listing_id = listing.id.clone();
                            }
                            self.settings.worlds.push(world);
                            self.save_settings();
                            self.adopt_sessions(self.settings.worlds.len() - 1);
                            self.refresh_channel_rules();
                        }
                    }
                }
                AppAction::SaveListing { id, tls, connect } => {
                    if let Some(i) = self.save_listing(&id, tls)
                        && connect
                    {
                        let endpoint = self.settings.worlds[i].endpoint();
                        self.open(endpoint, Some(i));
                    }
                }
                AppAction::NewWorld => self.edit_world(None),
                AppAction::MarkChannel(id, example, second) => {
                    self.open_mark_channel(id, &example, second.as_deref());
                }
                AppAction::OpenLink(url) => self.open_link(&url),
                AppAction::EditScripts(id) => self.edit_scripts(id),
                AppAction::ReloadScripts(id) => self.attach_library(id, true),
                AppAction::EditAgent(id) => self.edit_agent(id),
                AppAction::Notice(text) => self.notes.insert(0, text),
                AppAction::EditWorld(i) => {
                    if i < self.settings.worlds.len() {
                        self.edit_world(Some(i));
                    }
                }
                AppAction::DeleteWorld(i) => self.delete_world(i),
                AppAction::Focus(id) => self.focus_session(id),
                AppAction::Rename(id, name) => {
                    if let Some(e) = self.sessions.get_mut(id) {
                        e.tab.custom_name = (!name.trim().is_empty()).then(|| name.trim().to_string());
                    }
                }
                AppAction::SendCommand(id, command) => {
                    if let Some(e) = self.sessions.get_mut(id) {
                        e.tab.send_command(&command);
                    }
                }
                AppAction::Disconnect(id) => {
                    if let Some(e) = self.sessions.get_mut(id) {
                        e.tab.disconnect();
                    }
                }
                AppAction::Reconnect(id) => {
                    if let Some(e) = self.sessions.get_mut(id) {
                        e.tab.reconnect();
                        e.tab.focus_input = true;
                    }
                }
                AppAction::Close(id) => {
                    if let Some(e) = self.sessions.get_mut(id) {
                        e.tab.save_map(true);
                        e.tab.end_history();
                        self.history_drains.extend(e.tab.take_history_drains());
                    }
                    self.sessions.close(id);
                    self.channels.remove(&id);
                    self.maps.remove(&id);
                    self.full_maps.remove(&id);
                    let next = workspace::remove_document(&mut self.dock, Tab::Session(id));
                    if self.active_session == Some(id) {
                        self.active_session = self.sessions.iter().last().map(|e| e.tab.id);
                    }
                    self.show_document(next);
                    if self.sessions.is_empty() {
                        workspace::open_panel(&mut self.dock, Tab::Directory);
                        self.directory_active = true;
                    }
                }
                AppAction::CloseDirectory => {
                    let next = workspace::remove_document(&mut self.dock, Tab::Directory);
                    self.show_document(next);
                }
                AppAction::CloseOthers(keep) => {
                    for tab in workspace::documents(&self.dock) {
                        match tab {
                            t if t == keep => {}
                            Tab::Session(id) => self.actions.push(AppAction::Close(id)),
                            _ => self.actions.push(AppAction::CloseDirectory),
                        }
                    }
                    self.actions.push(match keep {
                        Tab::Session(id) => AppAction::Focus(id),
                        _ => AppAction::OpenDirectory,
                    });
                }
                AppAction::MoveTab(tab, to) => {
                    if workspace::move_document(&mut self.dock, tab, to) {
                        let order: Vec<SessionId> = workspace::documents(&self.dock)
                            .into_iter()
                            .filter_map(|t| match t {
                                Tab::Session(id) => Some(id),
                                _ => None,
                            })
                            .collect();
                        self.sessions.reorder(&order);
                    }
                }
                AppAction::Duplicate(id) => {
                    if let Some(e) = self.sessions.get(id) {
                        let (endpoint, world) = (e.tab.endpoint.clone(), e.tab.world);
                        if e.tab.is_demo() {
                            self.open_demo();
                        } else {
                            self.open(endpoint, world);
                        }
                    }
                }
                AppAction::MapDeleted(id, deleted) => {
                    let text = if deleted.rooms > 0 {
                        plural(deleted.rooms, S::ToastDeletedRoomsOne, S::ToastDeletedRoomsMany)
                    } else if deleted.labels > 0 {
                        t(S::ToastDeletedLabel).to_string()
                    } else {
                        plural(deleted.exits, S::ToastDeletedExitsOne, S::ToastDeletedExitsMany)
                    };
                    self.set_toast(crate::toast::Toast::new(
                        text,
                        crate::toast::Undo::Map {
                            session: id,
                            edit: deleted.edit,
                        },
                    ));
                }
                AppAction::UndoToast(serial) => self.undo_toast(serial),
                AppAction::OpenMapImport(id) => self.open_map_import(ctx, id),
                AppAction::OpenSettings => self.settings_dialog = Some(SettingsDialog::new(&self.settings)),
                AppAction::OpenMapEditor(id) => self.open_map_editor(id),
                AppAction::OpenFullMap(id) => self.open_full_map(id),
                AppAction::RememberSplit(share) => {
                    self.split_share = share;
                    self.save_layout(false);
                }
                AppAction::InstallClassifier(path) => self.install_classifier(path, ctx),
                AppAction::OpenDemo => self.open_demo(),
                AppAction::OpenDirectory => {
                    workspace::open_panel(&mut self.dock, Tab::Directory);
                    self.directory_active = true;
                }
                AppAction::OpenPanel(tab) if self.auto.is_hidden(tab) => self.auto.show(tab, true),
                AppAction::OpenPanel(tab) => workspace::open_panel(&mut self.dock, tab),
                AppAction::Unpin(tab) => {
                    if self.auto.unpin(&mut self.dock, tab, self.dock_rect) {
                        self.save_layout(true);
                    }
                }
                AppAction::Pin(tab) => {
                    if self.auto.pin(&mut self.dock, tab) {
                        self.save_layout(true);
                    }
                }
                AppAction::ResetLayout => self.reset_layout(),
                AppAction::RefreshDirectory => {
                    self.directory.refresh(true);
                    self.art.forget_failures();
                }
                AppAction::ShowAdult(on) => {
                    self.settings.show_adult = on;
                    self.save_settings();
                }
                AppAction::SettingsChanged => self.settings_changed(ctx),
                AppAction::CommandStyleTip(id, use_hash) => self.answer_style_tip(ctx, id, use_hash),
                AppAction::HideHistoryNotice => self.hide_history_notice(),
                AppAction::DismissNotice(id) => {
                    if let Some(e) = self.sessions.get_mut(id) {
                        e.tab.strip = None;
                    }
                }
                AppAction::OpenHistory => self.open_history(ctx),
            }
        }
        let title = match self.active_session.and_then(|id| self.sessions.get(id)) {
            Some(e) if !self.directory_active => e.tab.window_title(crate::dialogs::APP_NAME),
            _ => crate::dialogs::APP_NAME.into(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Which session the side panels follow and whether the directory is the shown document.
    fn track_focus(&mut self) {
        match self.dock.find_active_focused() {
            Some((_, Tab::Session(id))) => {
                self.active_session = Some(*id);
                self.directory_active = false;
            }
            Some((_, Tab::Directory)) => self.directory_active = true,
            _ => {}
        }
        if self.active_session.is_some_and(|id| self.sessions.get(id).is_none()) {
            self.active_session = workspace::visible_sessions(&self.dock).first().copied();
        }
        self.follow_active_world();
        if self.dock.find_tab(&Tab::Directory).is_none() {
            self.directory_active = false;
        }
    }

    /// Ctrl+Tab and Ctrl+Shift+Tab move through the session tabs in the order shown (Find a
    /// MUD among them while it is open), as C# moved through Find a MUD and the sessions;
    /// Cmd+1 to Cmd+9 (Ctrl elsewhere) show tab 1 to 9.
    fn cycle_views(&mut self, ctx: &egui::Context) {
        const DIGITS: [egui::Key; 9] = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        let blocked = self.dialog.is_some() || self.settings_dialog.is_some() || self.form.is_some();
        let (next, prev, jump) = ctx.input_mut(|i| {
            let jump = if blocked {
                None
            } else {
                DIGITS
                    .iter()
                    .position(|key| i.consume_key(egui::Modifiers::COMMAND, *key))
            };
            // Ctrl+Shift+Tab first: Ctrl+Tab's match ignores an extra Shift.
            let prev = i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Tab);
            (i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab), prev, jump)
        });
        if next || prev {
            self.step_views(prev);
        }
        if let Some(n) = jump
            && let Some(tab) = workspace::documents(&self.dock).get(n).copied()
        {
            self.actions.push(match tab {
                Tab::Session(id) => AppAction::Focus(id),
                _ => AppAction::OpenDirectory,
            });
        }
    }

    /// Move to the next (or previous) session tab, wrapping round.
    fn step_views(&mut self, back: bool) {
        let tabs = workspace::documents(&self.dock);
        if tabs.is_empty() {
            return;
        }
        let current = if self.directory_active {
            Tab::Directory
        } else {
            self.active_session.map_or(Tab::Directory, Tab::Session)
        };
        let at = tabs.iter().position(|t| *t == current).unwrap_or(0);
        let n = tabs.len();
        let to = if back { (at + n - 1) % n } else { (at + 1) % n };
        self.actions.push(match tabs[to] {
            Tab::Session(id) => AppAction::Focus(id),
            _ => AppAction::OpenDirectory,
        });
    }

    /// Show the document a closed tab handed over to.
    fn show_document(&mut self, next: Option<Tab>) {
        match next {
            Some(Tab::Session(id)) => self.focus_session(id),
            Some(Tab::Directory) => {
                workspace::focus_tab(&mut self.dock, Tab::Directory);
                self.directory_active = true;
            }
            _ => {}
        }
    }

    /// The toolbar, as the C# client's: the world picker with Connect and Disconnect on the
    /// left, Find a MUD and Settings on the right. The picker's list also takes a typed address.
    fn toolbar(&mut self, ui: &mut Ui) {
        let state = self.menu_state();
        ui.horizontal(|ui| {
            ui.set_min_height(34.0);
            self.world_picker(ui);
            ui.add_space(4.0);
            if widgets::tool_button(ui, Icon::Reconnect, None, &self.theme, state.world_selected)
                .on_hover_text(self.connect_hint())
                .clicked()
            {
                self.run_command(ui.ctx(), Command::ConnectSelected);
            }
            if widgets::tool_button(ui, Icon::Stop, None, &self.theme, state.can_disconnect)
                .on_hover_text(t(S::DisconnectTheActiveSession))
                .clicked()
            {
                self.run_command(ui.ctx(), Command::Disconnect);
            }
            self.toolbar_note(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::tool_button(ui, Icon::Gear, Some(t(S::SettingsTitle)), &self.theme, true).clicked() {
                    self.actions.push(AppAction::OpenSettings);
                }
                if widgets::tool_button(ui, Icon::Search, Some(t(S::FindAMUD)), &self.theme, true)
                    .on_hover_text(t(S::SearchTheDirectoryForAMUDServer))
                    .clicked()
                {
                    self.actions.push(AppAction::OpenDirectory);
                }
                ui.add_space(6.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, self.theme.border);
            });
        });
    }

    /// The System skin's toolbar, in the caption area: the icon and title at the left; the world
    /// picker, Connect, Disconnect, Find a MUD and the title buttons at the right (C#
    /// `ApplyWindowSkin` without custom chrome).
    fn system_toolbar(&mut self, ui: &mut Ui) -> Vec<TitleAction> {
        let state = self.menu_state();
        let mut actions = Vec::new();
        let mac = self.platform == Platform::Mac;
        ui.horizontal(|ui| {
            ui.set_min_height(34.0);
            if !mac {
                // Windows and Linux: the menu button first, at the top left.
                let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                self.draw_menu_button(ui, rect);
                ui.add_space(4.0);
            }
            // The window's own title in the caption area, as C# (character, world, app), not the
            // drawn skins' plate.
            let title = self.title.clone();
            ui.scope(|ui| {
                ui.set_max_width(26.0 + 9.0 + 300.0);
                title_bar::system_identity(ui, &title, &self.theme);
            });
            self.toolbar_note(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if mac {
                    // macOS: the menu button is the last title control.
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                    self.draw_menu_button(ui, rect);
                }
                let (slot, _) =
                    ui.allocate_exact_size(egui::vec2(title_bar::actions_width(1.0), 30.0), egui::Sense::hover());
                let follows = self.follows_world();
                actions = title_bar::actions(
                    ui,
                    slot.right(),
                    slot.center().y,
                    1.0,
                    &self.theme,
                    &self.chrome,
                    &self.settings,
                    follows,
                    &mut self.title_menu,
                    self.full_screen,
                );
                if widgets::tool_button(ui, Icon::Search, None, &self.theme, true)
                    .on_hover_text(t(S::SearchTheDirectoryForAMUDServer))
                    .clicked()
                {
                    self.actions.push(AppAction::OpenDirectory);
                }
                ui.add_space(4.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, self.theme.border);
                ui.add_space(4.0);
                // One icon language in the System toolbar (C# UI review, item 10): outlines.
                if widgets::tool_button(ui, Icon::StopOutline, None, &self.theme, state.can_disconnect)
                    .on_hover_text(t(S::DisconnectTheActiveSession))
                    .clicked()
                {
                    self.run_command(ui.ctx(), Command::Disconnect);
                }
                if widgets::tool_button(ui, Icon::Play, None, &self.theme, state.world_selected)
                    .on_hover_text(self.connect_hint())
                    .clicked()
                {
                    self.run_command(ui.ctx(), Command::ConnectSelected);
                }
                self.world_picker(ui);
            });
        });
        actions
    }

    /// The theme and skin on screen: the settings' (the open Settings dialog's draft while it
    /// previews), with the active session's world theme when world themes are allowed. Cheap
    /// when nothing changed; a change rebuilds the visuals and the skin's colours.
    fn refresh_appearance(&mut self, ctx: &egui::Context) {
        let source = self.settings_dialog.as_ref().map_or(&self.settings, |d| &d.draft);
        let world = self
            .active_session
            .and_then(|id| self.sessions.get(id))
            .and_then(|e| e.tab.world_theme.as_ref())
            .filter(|w| source.use_world_themes && w.is_valid());
        let theme = Theme::resolve(source, world);
        let skin = SkinId::from_name(&source.skin);
        let same_world = self.applied_world.as_ref() == world;
        if theme == self.theme && skin == self.skin && same_world {
            return;
        }
        let world = world.cloned();
        self.chrome = Chrome::new(&theme, skin, world.as_ref().and_then(|w| w.skin.as_ref()));
        self.chrome_generation += 1;
        theme.apply(ctx);
        self.theme = theme;
        self.skin = skin;
        self.applied_world = world;
        self.title_menu = self
            .title_menu
            .filter(|_| WindowSkin::of(skin).custom_chrome() || skin == SkinId::System);
    }

    /// A world's theme is on screen now (the palette menu checks no theme then).
    fn follows_world(&self) -> bool {
        self.applied_world.is_some()
    }

    /// The window's frame for the skin: the title band with its plate, the title buttons and
    /// (on Windows and Linux) the caption buttons; the frame around the content; the toolbar
    /// row. System keeps the OS title bar and puts a plain toolbar in the caption area.
    fn window_chrome(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.menu_button.button = None;
        let window = ctx.content_rect();
        let mac = self.platform == Platform::Mac;
        let skin = self.chrome.skin;
        // Windows and Linux: the OS frame only for System (and while full screen).
        if !mac {
            let want = !skin.custom_chrome() || self.full_screen;
            if self.decorations != Some(want) {
                self.decorations = Some(want);
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(want));
            }
        }
        let mut actions = Vec::new();
        self.title_plate = None;
        let drawn = skin.title_bar.filter(|_| !self.full_screen);
        let mut plate = None;
        if let Some(metrics) = drawn {
            let (left, right) = skin::caption_exclusion(mac, &metrics);
            let world = self.plate_world();
            let (full, short) = skin::plate_labels(world.as_deref());
            let room = skin::text_room(window.width(), left, right, metrics.text_inset)
                - metrics.logo_size
                - skin::TITLE_LOGO_GAP;
            let text = skin::plate_title(&full, &short, room, |s| title_bar::title_width(&ctx, s, &metrics));
            let identity = ((title_bar::title_width(&ctx, &text, &metrics) + metrics.logo_size + skin::TITLE_LOGO_GAP)
                / 2.0)
                .ceil()
                * 2.0;
            let place = skin::place_title(&metrics, window.width(), left, right, identity);
            let rect = place.rect.translate(window.min.to_vec2());
            let wear = (skin.id == SkinId::Armored).then(|| skin::wear_texture(&ctx)).flatten();
            // The band and frame change only with the window, the plate or the colours: baked
            // once into meshes and reused every frame.
            let key = skin::bake_key(
                &ctx,
                &[
                    window.min.x,
                    window.min.y,
                    window.max.x,
                    window.max.y,
                    rect.min.x,
                    rect.max.x,
                    rect.max.y,
                ],
                self.chrome_generation * 2 + u64::from(mac),
            );
            let chrome = &self.chrome;
            let shapes = self.baked_frame.get(&ctx, key, |canvas| {
                if skin.id == SkinId::Armored {
                    skin::paint_armored_frame(canvas, window, rect, (left, right), chrome, wear.as_ref());
                } else {
                    skin::paint_fleet_band(canvas, window, rect, chrome, mac);
                    skin::paint_fleet_edges(canvas, window, metrics.band_height, chrome);
                }
            });
            ui.painter().extend(shapes.iter().cloned());
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            let follows = self.follows_world();
            egui::Panel::top("title-band")
                .exact_size(metrics.band_height)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let band = ui.max_rect();
                    // macOS: the native traffic lights' corner is theirs, not the drag area's.
                    let hole = mac.then(|| skin::mac_lights_rect(&metrics).translate(window.min.to_vec2()));
                    title_bar::drag_area_around(ui, band, hole);
                    let mut right_edge = window.right() - skin::actions_right_inset(mac);
                    // The menu button: the last title control on macOS (the traffic lights hold
                    // the left), the first at the top left on Windows and Linux.
                    let size = 30.0 * metrics.action_scale;
                    let center_y = window.top() + metrics.actions_center();
                    let menu = if mac {
                        let rect = egui::Rect::from_center_size(
                            egui::pos2(right_edge - size / 2.0, center_y),
                            egui::vec2(size, size),
                        );
                        right_edge -= size + 4.0 * metrics.action_scale;
                        rect
                    } else {
                        egui::Rect::from_center_size(
                            egui::pos2(window.left() + skin.frame[0] + 8.0 + size / 2.0, center_y),
                            egui::vec2(size, size),
                        )
                    };
                    self.draw_menu_button(ui, menu);
                    actions = title_bar::actions(
                        ui,
                        right_edge,
                        window.top() + metrics.actions_center(),
                        metrics.action_scale,
                        &self.theme,
                        &self.chrome,
                        &self.settings,
                        follows,
                        &mut self.title_menu,
                        self.full_screen,
                    );
                    if !mac {
                        let height = if skin.id == SkinId::Armored {
                            metrics.band_height - 18.0
                        } else {
                            metrics.band_height
                        };
                        let area = egui::Rect::from_min_size(
                            egui::pos2(window.right() - skin::CAPTION_BUTTONS_WIDTH - 6.0, window.top() + 3.0),
                            egui::vec2(skin::CAPTION_BUTTONS_WIDTH, height - 6.0),
                        );
                        actions.extend(title_bar::caption_buttons(
                            ui,
                            area,
                            &self.theme,
                            &self.chrome,
                            maximized,
                        ));
                    }
                });
            let frame = skin.frame;
            for (id, size, side) in [
                ("skin-foot", frame[3], 0),
                ("skin-left", frame[0], 1),
                ("skin-right", frame[2], 2),
            ] {
                if size <= 0.0 {
                    continue;
                }
                let panel = match side {
                    0 => egui::Panel::bottom(id),
                    1 => egui::Panel::left(id),
                    _ => egui::Panel::right(id),
                };
                panel
                    .exact_size(size)
                    .resizable(false)
                    .show_separator_line(false)
                    .frame(egui::Frame::NONE)
                    .show(ui, |_| {});
            }
            plate = Some((rect, place.plain, text, metrics, wear));
        }
        if self.toolbar_visible {
            match drawn {
                Some(metrics) => {
                    egui::Panel::top("toolbar")
                        .exact_size(metrics.toolbar_min_height)
                        .resizable(false)
                        .show_separator_line(false)
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            let rect = ui.max_rect();
                            if let Some((plate_rect, ..)) = &plate {
                                let plate_rect = *plate_rect;
                                let key = skin::bake_key(
                                    &ctx,
                                    &[
                                        rect.min.x,
                                        rect.min.y,
                                        rect.max.x,
                                        rect.max.y,
                                        plate_rect.min.x,
                                        plate_rect.max.x,
                                        plate_rect.max.y,
                                    ],
                                    self.chrome_generation,
                                );
                                let chrome = &self.chrome;
                                let shapes = self.baked_toolbar.get(&ctx, key, |canvas| {
                                    if skin.id == SkinId::Armored {
                                        skin::paint_armored_toolbar(canvas, rect, plate_rect, chrome);
                                    } else {
                                        skin::paint_fleet_toolbar(canvas, rect, plate_rect, chrome);
                                    }
                                });
                                ui.painter().extend(shapes.iter().cloned());
                            }
                            let inner = egui::Rect::from_min_max(
                                egui::pos2(rect.left() + 12.0, rect.top() + metrics.toolbar_top_padding),
                                egui::pos2(rect.right() - 12.0, rect.bottom() - 5.0),
                            );
                            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| self.toolbar(ui));
                        });
                }
                None => {
                    let left = if mac && !self.full_screen {
                        skin::MAC_CAPTION_LEFT as i8
                    } else {
                        12
                    };
                    egui::Panel::top("toolbar")
                        .exact_size(skin::SYSTEM_TOOLBAR_HEIGHT)
                        .resizable(false)
                        .frame(
                            egui::Frame::new()
                                .fill(self.chrome.toolbar.middle())
                                .stroke(egui::Stroke::new(1.0, self.theme.border))
                                .inner_margin(egui::Margin {
                                    left,
                                    right: 12,
                                    top: 7,
                                    bottom: 7,
                                }),
                        )
                        .show(ui, |ui| {
                            // The row is the caption area: its empty parts move the window.
                            let row = ui.max_rect().expand2(egui::vec2(f32::from(left), 7.0));
                            title_bar::drag_area(ui, row);
                            let more = self.system_toolbar(ui);
                            actions.extend(more);
                        });
                }
            }
        } else if let Some(metrics) = drawn {
            // Without the toolbar, the content still clears the plate's projection.
            egui::Panel::top("toolbar-clearance")
                .exact_size(metrics.hidden_toolbar_clearance)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let rect = ui.max_rect();
                    skin::gradient_on(ui.painter(), rect, &self.chrome.toolbar);
                });
        } else if skin.id == SkinId::System && mac {
            // The caption area still holds the traffic lights.
            egui::Panel::top("caption-clearance")
                .exact_size(28.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(self.chrome.toolbar.middle()))
                .show(ui, |ui| title_bar::drag_area(ui, ui.max_rect()));
        }
        // The plate goes over the band and the toolbar's ledge it projects into.
        if let Some((rect, plain, text, metrics, wear)) = plate {
            let identity_area = if plain {
                rect
            } else {
                let key = skin::bake_key(
                    &ctx,
                    &[rect.min.x, rect.min.y, rect.max.x, rect.max.y],
                    self.chrome_generation,
                );
                let chrome = &self.chrome;
                let shapes = self.baked_plate.get(&ctx, key, |canvas| {
                    if skin.id == SkinId::Armored {
                        skin::paint_armored_plaque(canvas, rect, chrome, wear.as_ref());
                    } else {
                        skin::paint_fleet_plaque(canvas, rect, chrome);
                    }
                });
                ui.painter().extend(shapes.iter().cloned());
                if skin.id == SkinId::Armored {
                    skin::armored_well(rect)
                } else {
                    skin::fleet_plate(rect)
                }
            };
            let color = if plain { self.theme.text } else { self.chrome.title_text };
            title_bar::paint_identity(ui, identity_area, &text, &metrics, color);
            let full = skin::plate_labels(self.plate_world().as_deref()).0;
            ui.interact(rect, egui::Id::new("title-plate"), egui::Sense::hover())
                .on_hover_text(full);
            self.title_plate = Some(rect);
        }
        if !mac && skin.custom_chrome() && !self.full_screen {
            title_bar::resize_edges(&ctx);
        }
        for action in actions {
            self.title_action(&ctx, action);
        }
    }

    /// Full screen entered or left outside the app's command (the green button, Esc): the band
    /// and the traffic lights follow the window. Only a change in the reported state counts, so
    /// the app's own toggle is not undone while the window is still on its way.
    fn follow_full_screen(&mut self, ctx: &egui::Context) {
        let reported = ctx.input(|i| i.viewport().fullscreen);
        if reported != self.os_full_screen {
            self.os_full_screen = reported;
            if let Some(full) = reported {
                self.full_screen = full;
            }
        }
    }

    /// macOS: keeps the native traffic lights centred on a drawn skin's band (and back where
    /// AppKit puts them for System), re-applied after anything that may have reset them.
    fn place_traffic_lights(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        use crate::traffic_lights::{Trigger, Want, want};
        if self.platform != Platform::Mac {
            return;
        }
        let (size, focused, minimized) = ctx.input(|i| {
            let v = i.viewport();
            let size = v
                .inner_rect
                .map_or([0, 0], |r| [r.width().round() as i32, r.height().round() as i32]);
            (size, v.focused.unwrap_or(true), v.minimized.unwrap_or(false))
        });
        let full_screen = self.full_screen;
        let zoom = ctx.zoom_factor();
        let trigger = Trigger {
            skin: self.chrome.skin.id,
            size,
            focused,
            full_screen,
            minimized,
            theme: self.chrome_generation,
            zoom: zoom.to_bits(),
        };
        let poll = self.lights_schedule.poll(trigger, Instant::now());
        if let Some(after) = poll.wake_after {
            ctx.request_repaint_after(after);
        }
        if !poll.apply {
            return;
        }
        let wanted = match want(&self.chrome.skin, full_screen, minimized) {
            Want::Place(p) => Want::Place(p.scaled(zoom)),
            other => other,
        };
        #[cfg(target_os = "macos")]
        self.lights.apply(frame, wanted);
        #[cfg(not(target_os = "macos"))]
        let _ = (frame, wanted);
    }

    /// Carry out what the title bar asked.
    fn title_action(&mut self, ctx: &egui::Context, action: TitleAction) {
        match action {
            TitleAction::Skin(id) => self.set_skin(ctx, id),
            TitleAction::Theme(id) => {
                self.settings.theme = id;
                self.settings.use_world_themes = false;
                self.settings_changed(ctx);
            }
            TitleAction::ToggleWorldThemes => {
                self.settings.use_world_themes = !self.settings.use_world_themes;
                self.settings_changed(ctx);
            }
            TitleAction::FullScreen => self.run_command(ctx, Command::FullScreen),
            TitleAction::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            TitleAction::Maximize => {
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            TitleAction::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    /// Choose a window skin (the title bar's Skin menu, View > Skin): saved, and applied without
    /// touching sessions or the dock.
    fn set_skin(&mut self, ctx: &egui::Context, id: SkinId) {
        if self.settings.skin == id.name() {
            return;
        }
        self.settings.skin = id.name().into();
        self.settings_changed(ctx);
    }

    /// The world named on the title plate: the active session's.
    fn plate_world(&self) -> Option<String> {
        let tab = self.active_tab()?;
        let name = tab.world_name();
        (!name.is_empty()).then(|| name.to_string())
    }

    /// The connect error or the oldest note, beside the toolbar's buttons (click to dismiss).
    fn toolbar_note(&mut self, ui: &mut Ui) {
        if let Some(error) = &self.connect_error {
            ui.label(RichText::new(error).color(self.theme.error));
        } else if let Some(note) = self.notes.first() {
            let note = note.clone();
            if ui
                .label(RichText::new(&note).color(self.theme.warn))
                .on_hover_text(t(S::ClickToDismiss))
                .clicked()
            {
                self.notes.remove(0);
            }
        }
    }

    /// When the active session changes, the world picker shows its world (a session to an
    /// address that is no saved world shows nothing chosen). Choosing another world afterwards
    /// stays until the active session changes again.
    fn follow_active_world(&mut self) {
        if self.picker_session == self.active_session {
            return;
        }
        self.picker_session = self.active_session;
        if let Some(entry) = self.active_session.and_then(|id| self.sessions.get(id)) {
            self.panel.selected_world = entry.tab.world.filter(|&i| i < self.settings.worlds.len());
        }
    }

    /// Whether the picker shows the active session's own world.
    fn picker_on_active_world(&self) -> bool {
        self.panel.selected_world.is_some() && self.active_tab().and_then(|t| t.world) == self.panel.selected_world
    }

    /// The Connect button's hint: what it opens.
    fn connect_hint(&self) -> String {
        match self.panel.selected_world.and_then(|i| self.settings.worlds.get(i)) {
            Some(world) => tf(S::PickerConnectTo, &[&world.name]),
            None => t(S::ConnectToTheSelectedWorldInASessionTab).to_string(),
        }
    }

    /// The world picker: saved worlds, and an address typed into its list.
    fn world_picker(&mut self, ui: &mut Ui) {
        // The C# `toolbar-world-picker`: a quiet field in the shell colour, no outline.
        let theme = self.theme.clone();
        ui.scope(|ui| {
            let visuals = ui.visuals_mut();
            for w in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.open,
            ] {
                w.weak_bg_fill = theme.shell;
                w.bg_fill = theme.shell;
                w.bg_stroke = egui::Stroke::NONE;
                w.corner_radius = egui::CornerRadius::same(3);
            }
            visuals.widgets.hovered.weak_bg_fill = crate::theme::mix(theme.shell, theme.text, 0.05);
            ui.spacing_mut().interact_size.y = 30.0;
            self.world_picker_field(ui);
        });
    }

    fn world_picker_field(&mut self, ui: &mut Ui) {
        use crate::saved_worlds_panel as picker;
        {
            let selected = self.panel.selected_world.and_then(|i| self.settings.worlds.get(i));
            // The active session's own world is marked with its status lamp, so the picker reads
            // as "this session's world" until another world is chosen to connect to.
            let lamp = self.picker_on_active_world() && self.active_tab().is_some_and(|t| t.is_connected());
            let text: egui::WidgetText = match selected {
                Some(w) if lamp => {
                    let font = egui::FontId::proportional(14.0);
                    let mut job = egui::text::LayoutJob::default();
                    job.append("\u{25CF} ", 0.0, egui::TextFormat::simple(font.clone(), self.theme.ok));
                    job.append(&w.name, 0.0, egui::TextFormat::simple(font, self.theme.text));
                    job.into()
                }
                Some(w) => RichText::new(&w.name).size(14.0).color(self.theme.text).into(),
                None => RichText::new(t(S::ChooseAWorld))
                    .size(14.0)
                    .color(self.theme.muted)
                    .into(),
            };
            let mut picked = None;
            let mut connect_typed = false;
            let mut picker_field = egui::Rect::NOTHING;
            let catalog = self.directory_status.catalog.clone();
            let base = self.directory.base().to_string();
            let theme = self.theme.clone();
            // The popup stays open for clicks inside it (the address field takes focus); picking a
            // world or connecting closes it, as do a click outside and Escape.
            let picker = egui::ComboBox::from_id_salt("world-picker")
                .selected_text(text)
                .width(picker::PICKER_POPUP_WIDTH)
                .height(picker::PICKER_POPUP_MAX_HEIGHT)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    if !self.settings.worlds.is_empty() {
                        let ppp = ui.ctx().pixels_per_point();
                        let target = crate::artwork::Target {
                            width: (picker::PICKER_THUMB.x * ppp).round() as u32,
                            height: (picker::PICKER_THUMB.y * ppp).round() as u32,
                            cover: true,
                        };
                        // Six rows show at once; more scroll.
                        egui::ScrollArea::vertical()
                            .id_salt("world-picker-list")
                            .max_height(picker::PICKER_VISIBLE_ROWS * (picker::PICKER_ROW_HEIGHT + 2.0))
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                for (i, world) in self.settings.worlds.iter().enumerate() {
                                    let listing = picker::listing_of(catalog.as_deref(), world);
                                    let selected = self.panel.selected_world == Some(i);
                                    if picker::picker_row(
                                        ui,
                                        world,
                                        listing,
                                        &base,
                                        &mut self.art,
                                        target,
                                        &theme,
                                        selected,
                                    )
                                    .clicked()
                                    {
                                        picked = Some(i);
                                        ui.close();
                                    }
                                }
                            });
                        ui.separator();
                    }
                    ui.label(
                        RichText::new(t(S::ConnectToAnAddress))
                            .size(13.0)
                            .color(self.theme.muted),
                    );
                    ui.horizontal(|ui| {
                        let edit = egui::TextEdit::singleline(&mut self.address)
                            .hint_text(t(S::HostnameAcceptsHostPort))
                            .font(egui::FontId::proportional(14.0))
                            .desired_width(picker::PICKER_POPUP_WIDTH - 90.0);
                        let response = ui.add(edit);
                        picker_field = response.rect;
                        crate::a11y::label(&response, t(S::ConnectToAnAddress));
                        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button(RichText::new(t(S::ConnectShort)).size(14.0)).clicked() || enter {
                            connect_typed = true;
                            ui.close();
                        }
                    });
                    for address in &self.settings.recent {
                        if ui.selectable_label(false, RichText::new(address).size(13.0)).clicked() {
                            self.address = address.clone();
                        }
                    }
                    ui.add_space(4.0);
                })
                .response
                .on_hover_text(if self.picker_on_active_world() {
                    t(S::PickerActiveHint)
                } else {
                    t(S::PickerHint)
                });
            crate::a11y::label_combo(&picker, t(S::ChooseAWorld));
            self.picker_rects = (picker.rect, picker_field);
            if let Some(i) = picked {
                self.panel.selected_world = Some(i);
            }
            if connect_typed {
                match self.address.parse::<Endpoint>() {
                    Ok(endpoint) => {
                        self.connect_error = None;
                        self.actions.push(AppAction::Connect(endpoint));
                    }
                    Err(e) => self.connect_error = Some(e.to_string()),
                }
            }
        }
    }

    /// The session the menus act on: the one the side panels follow.
    fn active_tab(&self) -> Option<&SessionTab> {
        self.active_session.and_then(|id| self.sessions.get(id)).map(|e| &e.tab)
    }

    fn panel_visible(&self, tab: Tab) -> bool {
        self.auto.is_hidden(tab) || self.dock.find_tab(&tab).is_some()
    }

    /// What the menus need to know this frame.
    fn menu_state(&self) -> MenuState {
        use crate::terminal_view::Page;
        let active = self.active_tab();
        let session_view = self
            .active_session
            .and_then(|id| self.sessions.get(id))
            .map(|e| &e.view);
        let text_target = self.edit_target.is_some();
        MenuState {
            has_text: active.is_some_and(|tab| tab.terminal.has_text()),
            world_selected: self
                .panel
                .selected_world
                .is_some_and(|i| i < self.settings.worlds.len()),
            can_disconnect: active.is_some_and(|tab| !tab.is_closed()),
            text_target,
            can_copy: text_target || active.is_some_and(|tab| tab.terminal.selection_range().is_some()),
            can_select_all: text_target || active.is_some(),
            workspace_visible: self.panel_visible(Tab::Workspace),
            saved_worlds_visible: self.panel_visible(Tab::SavedWorlds),
            map_visible: self.panel_visible(Tab::Map),
            channels_visible: self.panel_visible(Tab::Channels),
            toolbar_visible: self.toolbar_visible,
            several_items: !self.sessions.is_empty(),
            full_screen: self.full_screen,
            skin: SkinId::ALL.iter().position(|id| *id == self.skin).unwrap_or(0) as u8,
            can_private: active.is_some_and(SessionTab::is_connected),
            manual_private: active.is_some_and(|tab| tab.manual_private),
            has_history: self.history_store.is_some(),
            has_scripts: active.is_some_and(|tab| tab.world.is_some()),
            has_session: active.is_some() && !self.directory_active,
            session_map: !self.directory_active && session_view.is_some_and(|v| v.page == Page::Map && !v.split),
            session_split: !self.directory_active
                && session_view.is_some_and(|v| v.split && v.page != Page::Diagnostics),
            map_undo: self.map_editing().is_some_and(|tab| tab.map.tracker().can_undo()),
            map_redo: self.map_editing().is_some_and(|tab| tab.map.tracker().can_redo()),
        }
    }

    /// The active session when its map editor is on screen (the Edit menu's Undo and Redo go to
    /// the map when no text field has the focus).
    fn map_editing(&self) -> Option<&SessionTab> {
        if self.directory_active || self.edit_target.is_some() {
            return None;
        }
        let entry = self.active_session.and_then(|id| self.sessions.get(id))?;
        let editing = entry.view.map_shown() && self.full_maps.get(&entry.tab.id).is_some_and(MapViewState::is_editor);
        editing.then_some(&entry.tab)
    }

    /// Show a tool panel, or close it if it is showing (View menu toggles).
    fn toggle_panel(&mut self, tab: Tab) {
        if self.auto.is_hidden(tab) {
            self.auto.pin(&mut self.dock, tab);
            if let Some(path) = self.dock.find_tab(&tab) {
                self.dock.remove_tab(path);
            }
        } else if let Some(path) = self.dock.find_tab(&tab) {
            self.dock.remove_tab(path);
        } else {
            workspace::open_panel(&mut self.dock, tab);
        }
        self.save_layout(true);
    }

    /// Open the offline demo world in a new session tab.
    fn open_demo(&mut self) {
        let options = self.tab_options(None, None);
        let id = self.sessions.open_demo(&options);
        self.attach_agent(id);
        workspace::add_document(&mut self.dock, Tab::Session(id));
        self.active_session = Some(id);
        self.directory_active = false;
    }

    /// File > Save Transcript: ask where, then write the active session's transcript.
    fn save_transcript(&mut self) {
        let Some(tab) = self.active_tab() else { return };
        let suggested = format!("wandur-{}.txt", file_stamp());
        let path = match &self.choose_file {
            Some(choose) => choose(&suggested),
            None => native_save_dialog(&suggested, self.data_dir.as_deref()),
        };
        let Some(path) = path else { return };
        if let Err(e) = tab.terminal.save_transcript(&path) {
            self.notes.insert(0, tf(S::CouldNotSaveTranscript, &[&e]));
        }
    }

    /// Open a link after the person confirmed it; a browser that does not open is a notice.
    fn open_link(&mut self, url: &str) {
        if !(self.launcher)(url) {
            self.notes.insert(0, t(S::LinkNotOpened).into());
        }
    }

    /// Run a menu command (from the menu bar, a shortcut or a toolbar button).
    fn run_command(&mut self, ctx: &egui::Context, command: Command) {
        let active = self.active_session.filter(|id| self.sessions.get(*id).is_some());
        match command {
            Command::FindAMud | Command::BrowseWorlds => self.actions.push(AppAction::OpenDirectory),
            Command::AddWorld => self.actions.push(AppAction::NewWorld),
            Command::OpenDemo => self.actions.push(AppAction::OpenDemo),
            Command::ImportMudlet => self.open_mudlet_import(),
            Command::ImportCsharp => self.open_csharp_import(ctx),
            Command::ImportMap => self.open_map_import(ctx, None),
            Command::SaveTranscript => self.save_transcript(),
            Command::CloseItem => {
                if self.directory_active {
                    self.actions.push(AppAction::CloseDirectory);
                } else if let Some(id) = active {
                    self.actions.push(AppAction::Close(id));
                }
            }
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Command::Undo | Command::Redo | Command::Cut | Command::Copy | Command::Paste | Command::SelectAll => {
                self.edit(ctx, command, active)
            }
            Command::Preferences => self.actions.push(AppAction::OpenSettings),
            Command::ToggleWorkspace => self.toggle_panel(Tab::Workspace),
            Command::ToggleSavedWorlds => self.toggle_panel(Tab::SavedWorlds),
            Command::ToggleMap => self.toggle_panel(Tab::Map),
            Command::ToggleChannels => self.toggle_panel(Tab::Channels),
            Command::RestorePanels => self.actions.push(AppAction::ResetLayout),
            Command::Layout(i) => {
                if let Some(preset) = workspace::Preset::ALL.get(i as usize) {
                    self.apply_preset(*preset);
                }
            }
            Command::ToggleToolbar => self.toolbar_visible = !self.toolbar_visible,
            Command::FocusInput => {
                if let Some(id) = active {
                    self.actions.push(AppAction::Focus(id));
                }
            }
            Command::ConnectSelected => {
                if let Some(i) = self.panel.selected_world.filter(|&i| i < self.settings.worlds.len()) {
                    self.actions.push(AppAction::ConnectWorld(i));
                }
            }
            Command::Disconnect => {
                if let Some(id) = active {
                    self.actions.push(AppAction::Disconnect(id));
                }
            }
            Command::ClearTranscript => {
                if let Some(e) = active.and_then(|id| self.sessions.get_mut(id)) {
                    e.tab.clear_transcript();
                }
            }
            Command::NextItem => self.step_views(false),
            Command::PreviousItem => self.step_views(true),
            Command::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Command::FullScreen => {
                self.full_screen = !self.full_screen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.full_screen));
            }
            Command::GettingStarted => {
                self.dialog = Some(Dialog::Link(wandur_core::site::help(&self.site)));
            }
            Command::OtherClients => {
                self.dialog = Some(Dialog::Link(wandur_core::site::other_clients(&self.site)));
            }
            Command::About => self.dialog = Some(Dialog::About),
            Command::CheckForUpdates => self.check_for_updates_now(ctx),
            Command::PrivateInput => {
                if let Some(e) = self.active_session.and_then(|id| self.sessions.get_mut(id))
                    && e.tab.is_connected()
                {
                    let on = !e.tab.manual_private;
                    e.tab.set_manual_private(on);
                }
            }
            Command::SessionHistory => self.actions.push(AppAction::OpenHistory),
            // Later tasks: skins (t15), scripts (t09), update checks (t16). Their items are
            // disabled.
            Command::SessionMap => self.toggle_session_map(),
            Command::SessionSplit => self.toggle_session_split(),
            Command::Scripts => {
                if let Some(id) = active {
                    self.actions.push(AppAction::EditScripts(id));
                }
            }
            Command::Skin(i) => {
                if let Some(id) = SkinId::ALL.get(usize::from(i)) {
                    self.set_skin(ctx, *id);
                }
            }
        }
    }

    /// The Edit menu: hand the command to the text field that had the focus (next frame, with
    /// the focus given back), or act on the active transcript.
    fn edit(&mut self, ctx: &egui::Context, command: Command, active: Option<SessionId>) {
        use egui::{Event, Key, Modifiers};
        let key = |key: Key, modifiers: Modifiers| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        if let Some(target) = self.edit_target {
            ctx.memory_mut(|m| m.request_focus(target));
            match command {
                Command::Undo => self.pending_edit.push(key(Key::Z, Modifiers::COMMAND)),
                Command::Redo => self
                    .pending_edit
                    .push(key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT)),
                Command::Cut => self.pending_edit.push(Event::Cut),
                Command::Copy => self.pending_edit.push(Event::Copy),
                Command::Paste => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
                Command::SelectAll => self.pending_edit.push(key(Key::A, Modifiers::COMMAND)),
                _ => {}
            }
            return;
        }
        if matches!(command, Command::Undo | Command::Redo)
            && let Some(id) = self.map_editing().map(|tab| tab.id)
            && let Some(editor) = self.full_maps.get_mut(&id).and_then(|m| m.editor.as_deref_mut())
        {
            editor.pending.push(if command == Command::Undo {
                crate::map_view::editor::EditAction::Undo
            } else {
                crate::map_view::editor::EditAction::Redo
            });
            ctx.request_repaint();
            return;
        }
        let Some(entry) = active.and_then(|id| self.sessions.get_mut(id)) else {
            return;
        };
        match command {
            Command::Copy => {
                if let Some(text) = entry.tab.terminal.selection_text() {
                    ctx.copy_text(text);
                }
            }
            Command::SelectAll => entry.tab.terminal.select_all(),
            _ => {}
        }
    }

    /// Remember the text field with the keyboard focus, for the Edit menu.
    fn track_edit_target(&mut self, ctx: &egui::Context) {
        if self.menu_button.open {
            return;
        }
        self.edit_target = ctx
            .memory(|m| m.focused())
            .filter(|id| egui::TextEdit::load_state(ctx, *id).is_some());
    }

    /// Measure time and allocation per panel from now on (the shell bench).
    #[doc(hidden)]
    pub fn panel_stats_mut(&mut self) -> &mut shell::PanelStats {
        self.panel_stats.get_or_insert_with(Default::default)
    }

    /// Worlds in the directory catalog (the shell bench waits for it).
    #[doc(hidden)]
    pub fn catalog_len(&self) -> usize {
        self.directory_status.catalog.as_ref().map_or(0, |c| c.len())
    }

    /// Scroll Find a MUD's results to `offset` points on the next frame (the shell bench).
    #[doc(hidden)]
    pub fn scroll_directory(&mut self, offset: f32) {
        self.dir_view.scroll_to = Some(offset);
    }

    /// The open sessions (the shell bench fills their panels).
    #[doc(hidden)]
    pub fn sessions_mut(&mut self) -> &mut Sessions {
        &mut self.sessions
    }

    /// Do what a click would ask (bench tests drive the app this way).
    #[doc(hidden)]
    pub fn push_action(&mut self, action: AppAction) {
        self.actions.push(action);
    }

    /// The saved worlds.
    #[doc(hidden)]
    pub fn saved_worlds(&self) -> &[SavedWorld] {
        &self.settings.worlds
    }

    /// The directory's listing by id, as the catalog holds it now.
    #[doc(hidden)]
    pub fn listing(&self, id: &str) -> Option<wandur_core::directory::listing::WorldListing> {
        self.directory_status.catalog.as_ref()?.by_id(id).cloned()
    }

    /// A saved world's script library, as the world editor would load it.
    #[doc(hidden)]
    pub fn world_library(&mut self, index: usize) -> Vec<LibraryEntry> {
        let id = self
            .settings
            .worlds
            .get(index)
            .map(|w| w.world_id.clone())
            .unwrap_or_default();
        self.library(&id)
    }

    /// Save a world's library as Save world in the editor does, and wait until it is written.
    #[doc(hidden)]
    pub fn save_world_library(&mut self, index: usize, after: Vec<LibraryEntry>) {
        let id = self
            .settings
            .worlds
            .get(index)
            .map(|w| w.world_id.clone())
            .unwrap_or_default();
        let before = self.library(&id);
        self.save_library(&id, LibraryChange { before, after });
        if let Some(writer) = &self.db_writer {
            writer.flush();
        }
    }

    /// The update check's schedule, and a finished check: its record saved, the menu command's
    /// answer shown.
    fn run_updates(&mut self, ctx: &egui::Context, now: Instant) {
        let wake = repaint_waker(ctx);
        self.updates.tick(now, &self.settings, Arc::clone(&wake));
        if let Some(next) = self.updates.next_due() {
            ctx.request_repaint_after(next.saturating_duration_since(now).max(Duration::from_millis(1)));
        }
        self.finish_update_check(ctx, false);
    }

    fn finish_update_check(&mut self, _ctx: &egui::Context, block: bool) {
        let Some(finished) = self.updates.poll(&self.settings, block) else {
            return;
        };
        if let Some(record) = finished.record {
            self.settings.last_update_check = Some(record);
            self.save_settings();
        }
        if let Some((heading, message)) = finished.answer {
            self.dialog = Some(Dialog::Information { heading, message });
        }
    }

    /// Help > Check for Updates: ask now. A newer release shows the strip; any other answer
    /// opens a dialog when the check finishes.
    fn check_for_updates_now(&mut self, ctx: &egui::Context) {
        if let Some((heading, message)) = self.updates.check_now(&self.settings, repaint_waker(ctx)) {
            self.dialog = Some(Dialog::Information { heading, message });
        }
    }

    /// Wait for a running update check and apply it (tests).
    #[doc(hidden)]
    pub fn wait_update_check(&mut self, ctx: &egui::Context) {
        self.finish_update_check(ctx, true);
    }

    /// Run the menu command Check for Updates and wait for its answer (tests).
    #[doc(hidden)]
    pub fn check_for_updates_and_wait(&mut self, ctx: &egui::Context) {
        self.check_for_updates_now(ctx);
        self.wait_update_check(ctx);
    }

    /// The automatic check as the schedule runs it when it is time (tests).
    #[doc(hidden)]
    pub fn automatic_update_check(&mut self, ctx: &egui::Context) {
        self.updates.look_now();
        self.updates.tick(Instant::now(), &self.settings, repaint_waker(ctx));
        self.wait_update_check(ctx);
    }

    /// The release the strip offers, whether the strip shows, and the dialog on screen (tests).
    #[doc(hidden)]
    pub fn update_state(&self) -> (Option<String>, bool, Option<Dialog>) {
        (
            self.updates.offered.as_ref().map(|o| o.version.clone()),
            self.updates.visible(self.active_private()),
            self.dialog.clone(),
        )
    }

    fn active_private(&self) -> bool {
        self.active_session
            .and_then(|id| self.sessions.get(id))
            .is_some_and(|e| e.tab.private_input())
    }

    /// The update strip under the toolbar, unless the active session's input is private.
    fn update_strip(&mut self, ui: &mut Ui) {
        if let Some(id) = self.updates.restore_focus.take() {
            ui.memory_mut(|m| m.request_focus(id));
        }
        if !self.updates.visible(self.active_private()) {
            return;
        }
        match self.updates.strip(ui, &self.theme) {
            None => {}
            Some(NoticeAction::Open(url)) => self.open_link(&url),
            Some(NoticeAction::Dismiss) => self.updates.dismissed = true,
            Some(NoticeAction::Skip(version)) => {
                self.updates.skip();
                self.settings.skipped_update_version = Some(version);
                self.save_settings();
            }
        }
    }

    /// Official maps (GMCP `Client.Map`): start an offer when a session's server names its map,
    /// take what the offers' workers sent, and merge a downloaded map into its session's map
    /// once that map is loaded. Without a data directory to keep the file in, nothing is offered.
    fn run_official_maps(&mut self, ctx: &egui::Context) {
        let store = match (&self.saver, &self.data_dir) {
            (Some(_), Some(dir)) => Some(wandur_core::map::official::store::OfficialStore::new(dir)),
            _ => None,
        };
        let mut ready = Vec::new();
        for entry in self.sessions.iter_mut() {
            let tab = &mut entry.tab;
            if let Some(url) = tab.client_map.take()
                && let Some(store) = &store
                && let Some(world) = tab.map_world()
                && tab.official_map.as_ref().is_none_or(|o| o.url != url)
            {
                let key = wandur_core::map::official::store::world_key(world);
                let not_now = tab.official_map.as_ref().is_some_and(|o| o.not_now);
                tab.official_map = Some(crate::official_map::OfficialMap::new(
                    &url,
                    &key,
                    store.clone(),
                    not_now,
                    Arc::clone(&self.official_fetch),
                    repaint_waker(ctx),
                ));
            }
            let loaded = tab.map.is_loaded();
            if let Some(offer) = &mut tab.official_map {
                offer.poll();
                if loaded && let Some(r) = offer.take_ready() {
                    ready.push((tab.id, r));
                }
            }
        }
        for (id, r) in ready {
            self.merge_official_map(id, r);
        }
    }

    /// Merge a downloaded official map into a session's map as one undoable step; the toast
    /// says what changed and offers it back. The file is then kept as the next merge's base.
    fn merge_official_map(&mut self, id: SessionId, ready: Box<crate::official_map::Ready>) {
        let Some(entry) = self.sessions.get_mut(id) else {
            return;
        };
        entry.tab.stop_walk();
        let result = wandur_core::map::official::merge::merge(
            entry.tab.map.tracker_mut(),
            ready.base.as_ref(),
            &ready.prepared.map,
        );
        for state in [self.full_maps.get_mut(&id), self.maps.get_mut(&id)]
            .into_iter()
            .flatten()
        {
            crate::map_view::editor::reset_after_import(state);
        }
        if let Some(inference) = &mut entry.tab.inference {
            inference.rescan();
        }
        let title = entry.tab.title().to_string();
        let edit = entry.tab.map.tracker().last_edit();
        let Some(offer) = &mut entry.tab.official_map else {
            return;
        };
        match result {
            Ok(report) => {
                offer.keep(ready, report);
                if !report.changed() {
                    self.notes.insert(0, t(S::MapImportNothingNew).to_string());
                    return;
                }
                let text = tf(
                    S::OfficialMapImported,
                    &[
                        &title,
                        &report.added,
                        &report.updated,
                        &report.kept_local,
                        &report.removed,
                    ],
                );
                if let Some(edit) = edit {
                    self.set_toast(Toast::new(text, Undo::Map { session: id, edit }));
                }
                self.actions.push(AppAction::OpenFullMap(id));
            }
            Err(e) => offer.phase = crate::official_map::Phase::Message(tf(S::OfficialMapFailed, &[&e.0])),
        }
    }

    /// The official map strip for the shown session: Download, Not now, Never (or a message).
    fn official_map_strip(&mut self, ui: &mut Ui) {
        use crate::official_map::StripAction;
        let Some(id) = self.active_session.filter(|_| !self.directory_active) else {
            return;
        };
        let Some(offer) = self
            .sessions
            .get_mut(id)
            .and_then(|e| e.tab.official_map.as_mut())
            .filter(|o| o.visible())
        else {
            return;
        };
        match crate::official_map::strip(ui, &offer.phase, &self.theme) {
            None => {}
            Some(StripAction::Download) => offer.download(),
            Some(StripAction::NotNow) => offer.not_now(),
            Some(StripAction::Dismiss) => offer.dismiss(),
            Some(StripAction::Never) => {
                if let Err(e) = offer.never() {
                    self.notes.insert(0, tf(S::OfficialMapPreferenceFailed, &[&e]));
                }
            }
        }
    }

    /// The strips of unpinned panels on the left, right and bottom edges, with a tab per panel:
    /// hovering one slides its panel out, clicking keeps it out.
    /// The notice strip under the toolbar for the shown session (the C# notice bar): the
    /// history reminder with Don't show again, or another message; × dismisses it.
    fn notice_strip(&mut self, ui: &mut Ui) {
        let Some(id) = self.active_session.filter(|_| !self.directory_active) else {
            return;
        };
        let Some(notice) = self.sessions.get(id).and_then(|e| e.tab.strip.clone()) else {
            return;
        };
        let theme = &self.theme;
        let mut hide = false;
        let mut dismiss = false;
        egui::Panel::top("notice-strip")
            .frame(
                egui::Frame::new()
                    .fill(theme.panel)
                    .stroke(egui::Stroke::new(1.0, theme.border))
                    .inner_margin(egui::Margin {
                        left: 24,
                        right: 14,
                        top: 6,
                        bottom: 6,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(26.0);
                    let text = match &notice {
                        StripNotice::HistoryRecording => t(S::HistoryRecordingNotice),
                        StripNotice::Text(text) => text.as_str(),
                    };
                    ui.label(RichText::new(text).size(13.0).color(theme.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("×").size(14.0)).min_size(egui::vec2(30.0, 26.0)))
                            .on_hover_text(t(S::ClickToDismiss))
                            .clicked()
                        {
                            dismiss = true;
                        }
                        ui.add_space(8.0);
                        if notice == StripNotice::HistoryRecording {
                            let mut checked = false;
                            if ui.checkbox(&mut checked, t(S::DontShowAgain)).changed() && checked {
                                hide = true;
                            }
                        }
                    });
                });
            });
        if hide {
            self.actions.push(AppAction::HideHistoryNotice);
        }
        if dismiss {
            self.actions.push(AppAction::DismissNotice(id));
        }
    }

    fn strips(&mut self, ui: &mut Ui) {
        self.strip_tabs.clear();
        for edge in [Edge::Left, Edge::Right, Edge::Bottom] {
            let tabs: Vec<Tab> = self.auto.on_edge(edge).map(|h| h.tab).collect();
            if tabs.is_empty() {
                continue;
            }
            let id = format!("autohide-{edge:?}");
            let panel = match edge {
                Edge::Left => egui::Panel::left(id),
                Edge::Right => egui::Panel::right(id),
                Edge::Bottom => egui::Panel::bottom(id),
            };
            let frame = egui::Frame::new()
                .fill(self.theme.shell)
                .inner_margin(egui::Margin::same(2));
            panel
                .exact_size(autohide::STRIP)
                .resizable(false)
                .frame(frame)
                .show(ui, |ui| {
                    let mut place = |ui: &mut Ui| {
                        for tab in &tabs {
                            let r = strip_tab(
                                ui,
                                *tab,
                                edge,
                                &self.theme,
                                self.auto.shown.is_some_and(|s| s.tab == *tab),
                            );
                            if r.clicked() {
                                self.auto.show(*tab, true);
                            } else if r.hovered() && self.auto.shown.is_none_or(|s| s.tab != *tab) {
                                self.auto.show(*tab, false);
                            }
                            self.strip_tabs.push((*tab, r.rect));
                        }
                    };
                    if edge == Edge::Bottom {
                        ui.horizontal(|ui| place(ui));
                    } else {
                        ui.vertical(|ui| place(ui));
                    }
                });
        }
    }

    /// Hide the slid-out panel on Escape, a click elsewhere, or (when opened by hovering) once
    /// the pointer has been away for a moment.
    fn track_overlay(&mut self, ctx: &egui::Context, overlay: Option<(egui::Rect, bool)>) {
        let Some(shown) = self.auto.shown else { return };
        let Some((rect, hide)) = overlay else {
            self.auto.hide();
            return;
        };
        if hide {
            self.auto.hide();
            return;
        }
        let strip = self.strip_tabs.iter().find(|(t, _)| *t == shown.tab).map(|(_, r)| *r);
        let (pointer, pressed, escape) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.any_pressed(),
                i.key_pressed(egui::Key::Escape),
            )
        });
        let inside = pointer.is_some_and(|p| rect.contains(p) || strip.is_some_and(|s| s.contains(p)));
        // A popup (a combo box or context menu in the panel) counts as inside.
        let popup = egui::Popup::is_any_open(ctx);
        if escape || (pressed && !inside && !popup) {
            self.auto.hide();
            return;
        }
        if let Some(wait) = self.auto.track_pointer(inside || popup, Instant::now()) {
            ctx.request_repaint_after(wait);
        }
    }

    /// The directory, the side panels and the probe see the same numbers.
    fn probe_input(&self) -> ProbeInput {
        ProbeInput {
            connected_sessions: self.sessions.connected(),
            chars_total: self.sessions.chars_total,
            frames_presented: self.frames_presented,
            catalog_worlds: self.directory_status.catalog.as_ref().map_or(0, |c| c.len()),
            scroll: (
                self.dir_view.scroll.offset,
                self.dir_view.scroll.viewport,
                self.dir_view.scroll.extent,
            ),
            cards_drawn: self.dir_view.cards_drawn,
        }
    }
}

/// One tab on an auto-hide strip: the panel's name, written downwards on the side strips.
fn strip_tab(ui: &mut Ui, tab: Tab, edge: Edge, theme: &Theme, shown: bool) -> egui::Response {
    let color = if shown { theme.accent } else { theme.text };
    let galley = ui
        .painter()
        .layout_no_wrap(tab.label().to_string(), egui::FontId::proportional(12.0), color);
    let text = galley.size();
    let size = match edge {
        Edge::Bottom => egui::vec2(text.x + 20.0, autohide::STRIP - 4.0),
        _ => egui::vec2(autohide::STRIP - 4.0, text.x + 20.0),
    };
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    crate::a11y::toggle(&response, egui::accesskit::Role::Tab, tab.label(), shown);
    let response = response.on_hover_text(tf(S::UnpinnedHint, &[&tab.label()]));
    if shown || response.hovered() {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(3), theme.hover_fill());
    }
    // A line on the inner side, like the tab of a docked panel.
    let stroke = egui::Stroke::new(2.0, if shown { theme.accent } else { theme.border });
    match edge {
        Edge::Left => ui.painter().vline(rect.right() - 1.0, rect.y_range(), stroke),
        Edge::Right => ui.painter().vline(rect.left() + 1.0, rect.y_range(), stroke),
        Edge::Bottom => ui.painter().hline(rect.x_range(), rect.top() + 1.0, stroke),
    };
    let shape = match edge {
        Edge::Bottom => egui::epaint::TextShape::new(rect.center() - text / 2.0, galley, color),
        _ => egui::epaint::TextShape::new(
            egui::pos2(rect.center().x + text.y / 2.0, rect.top() + 10.0),
            galley,
            color,
        )
        .with_angle(std::f32::consts::FRAC_PI_2),
    };
    ui.painter().add(shape);
    response
}

/// The slid-out panel over the dock, against its edge: a header with Pin and Hide, then the panel.
/// Returns its rectangle and whether Hide was clicked, or `None` if the panel is no longer unpinned.
fn overlay(
    ctx: &egui::Context,
    auto: &AutoHide,
    tab: Tab,
    dock: egui::Rect,
    theme: &Theme,
    viewer: &mut Viewer<'_>,
) -> Option<(egui::Rect, bool)> {
    use egui_dock::TabViewer as _;
    let hidden = auto.hidden.iter().find(|h| h.tab == tab)?;
    if !dock.is_positive() {
        return None;
    }
    let slide = ctx.animate_bool_with_time(egui::Id::new(("autohide-slide", tab)), true, 0.12);
    let rect = match hidden.edge {
        Edge::Left => {
            let w = hidden.size.min(dock.width() * 0.8);
            egui::Rect::from_min_size(
                dock.min - egui::vec2((1.0 - slide) * w, 0.0),
                egui::vec2(w, dock.height()),
            )
        }
        Edge::Right => {
            let w = hidden.size.min(dock.width() * 0.8);
            egui::Rect::from_min_size(
                egui::pos2(dock.right() - w + (1.0 - slide) * w, dock.top()),
                egui::vec2(w, dock.height()),
            )
        }
        Edge::Bottom => {
            let h = hidden.size.min(dock.height() * 0.8);
            egui::Rect::from_min_size(
                egui::pos2(dock.left(), dock.bottom() - h + (1.0 - slide) * h),
                egui::vec2(dock.width(), h),
            )
        }
    };
    if slide < 1.0 {
        ctx.request_repaint();
    }
    let mut tab_mut = tab;
    let mut hide = false;
    egui::Area::new(egui::Id::new(("autohide-overlay", tab)))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(rect.intersect(dock.expand(2.0)));
            egui::Frame::new()
                .fill(theme.panel)
                .stroke(egui::Stroke::new(1.0, theme.border))
                .shadow(ui.visuals().popup_shadow)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    let inner = rect.size() - egui::vec2(12.0, 12.0);
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(viewer.title(&mut tab_mut).text()).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(t(S::HideButton))
                                .on_hover_text(t(S::EscapeHidesIt))
                                .clicked()
                            {
                                hide = true;
                            }
                            if ui.small_button(t(S::PinButton)).on_hover_text(t(S::PinHint)).clicked() {
                                viewer.actions.push(AppAction::Pin(tab));
                            }
                        });
                    });
                    ui.separator();
                    viewer.ui(ui, &mut tab_mut);
                });
        });
    Some((rect, hide))
}

fn default_launcher() -> Launcher {
    Arc::new(|url: &str| webbrowser::open(url).is_ok())
}

/// A local date and time for file names: `2026-10-08-1430` (UTC where the local offset is unknown).
fn file_stamp() -> String {
    let now = unix_now() as i64 + local_offset_seconds();
    let (y, m, d) = wandur_core::directory::time::civil_from_days(now.div_euclid(86_400));
    let minutes = now.rem_euclid(86_400) / 60;
    format!("{y:04}-{m:02}-{d:02}-{:02}{:02}", minutes / 60, minutes % 60)
}

/// The local time's offset from UTC in seconds, now.
#[cfg(unix)]
pub(crate) fn local_offset_seconds() -> i64 {
    let now = unix_now() as libc::time_t;
    // SAFETY: localtime_r writes into the zeroed struct we own and reads only `now`.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            0
        } else {
            tm.tm_gmtoff as i64
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn local_offset_seconds() -> i64 {
    0
}

/// The native picker for a Mudlet profile folder or file (none without native dialogs).
#[cfg(feature = "native-dialogs")]
fn pick_mudlet_source(pick: crate::dialogs::MudletPick) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new().set_title(t(S::MudletImportTitle));
    match pick {
        crate::dialogs::MudletPick::Folder => dialog.pick_folder(),
        crate::dialogs::MudletPick::File => dialog
            .add_filter(t(S::MudletImportFileType), &["xml", "mpackage", "zip"])
            .pick_file(),
    }
}

#[cfg(not(feature = "native-dialogs"))]
fn pick_mudlet_source(_: crate::dialogs::MudletPick) -> Option<PathBuf> {
    None
}

/// The native Save dialog for a transcript.
#[cfg(feature = "native-dialogs")]
fn native_save_dialog(suggested: &str, _data_dir: Option<&std::path::Path>) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(t(S::SaveSessionTranscript))
        .set_file_name(suggested)
        .add_filter(t(S::TextFile), &["txt"])
        .save_file()
}

/// Without native dialogs a transcript goes to the data directory's `transcripts` folder.
#[cfg(not(feature = "native-dialogs"))]
fn native_save_dialog(suggested: &str, data_dir: Option<&std::path::Path>) -> Option<PathBuf> {
    let dir = data_dir?.join("transcripts");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(suggested))
}

fn default_fetcher(install: &InstallHeader) -> Arc<dyn Fetcher> {
    Arc::new(wandur_core::directory::client::HttpFetcher::new().with_install(install.clone()))
}

/// The update check for this run: `source` (tests and scenes) or `{base}/client/latest`, as
/// `version` (tests and scenes) or this build.
fn update_service(
    source: Option<&Arc<dyn wandur_core::updates::UpdateSource>>,
    version: Option<&str>,
    clock: Option<&wandur_core::updates::Clock>,
    base: &str,
    install: &InstallHeader,
) -> wandur_core::updates::UpdateService {
    use wandur_core::updates::{HttpUpdateSource, UpdateService, UpdateSource};
    let source: Arc<dyn UpdateSource> = match source {
        Some(source) => Arc::clone(source),
        None => Arc::new(HttpUpdateSource::new(
            base,
            wandur_core::directory::client::HttpFetcher::new().with_install(install.clone()),
        )),
    };
    let clock = clock.cloned().unwrap_or_else(wandur_core::updates::system_clock);
    match version {
        Some(version) => UpdateService::new(source, clock, version),
        None => UpdateService::for_this_build(source, clock),
    }
}

fn start_directory(
    setting: &str,
    dir: Option<&std::path::Path>,
    fetcher: &Arc<dyn Fetcher>,
    ctx: &egui::Context,
) -> DirectoryService {
    let env = std::env::var("WANDUR_DIRECTORY_URL").ok();
    let base = resolve_base(env.as_deref(), setting);
    DirectoryService::start(
        base,
        dir.map(std::path::Path::to_path_buf),
        Arc::clone(fetcher),
        repaint_waker(ctx),
    )
}

impl eframe::App for WandurApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.log_repaints {
            eprintln!(
                "frame {} at {:?}, causes {:?}",
                ctx.cumulative_frame_nr(),
                self.pacer.epoch.elapsed(),
                ctx.repaint_causes()
            );
        }
        // All network output since the last frame, applied once, for every session, plus the
        // reconnect and prompt timers.
        let now = Instant::now();
        if self.sessions.pump_all(now) > 0 {
            self.pacer.drained(ctx);
        }
        if let Some(deadline) = self.sessions.deadline() {
            ctx.request_repaint_after(deadline.saturating_duration_since(now).max(Duration::from_millis(1)));
        }
        self.run_updates(ctx, now);
        self.run_official_maps(ctx);
        if self.directory.revision() != self.directory_status.revision {
            self.directory_status = self.directory.status();
            self.refresh_session_mappings();
        }
        self.run_scene_input(ctx);
        self.run_map_scenes();
        self.run_map_import_scene(ctx);
        self.run_scene_tabs();
        if let Some(shot) = &mut self.screenshot
            && shot.on_frame(ctx)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let input = self.probe_input();
        if let Some(probe) = &mut self.probe {
            match probe.on_frame(ctx, input) {
                ProbeAction::None => {}
                ProbeAction::OpenSessions(endpoint, count) => {
                    for _ in 0..count {
                        self.open(endpoint.clone(), None);
                    }
                }
                ProbeAction::ShowDirectory => {
                    workspace::open_panel(&mut self.dock, Tab::Directory);
                    self.directory_active = true;
                }
                ProbeAction::ScrollDirectory(offset) => self.dir_view.scroll_to = Some(offset),
                ProbeAction::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        self.cycle_views(ctx);
        // Edit menu events reach the text field that had the focus (it has it again now).
        if !self.pending_edit.is_empty() {
            let events = std::mem::take(&mut self.pending_edit);
            ctx.input_mut(|i| i.events.extend(events));
        }
        #[cfg(target_os = "macos")]
        if let Some(bar) = &self.native_menu {
            for command in bar.take_commands() {
                self.run_command(ctx, command);
            }
        }
        self.menu_keys(ctx);
        // Window shortcuts, unless a dialog is open.
        if self.dialog.is_none() && self.settings_dialog.is_none() && self.form.is_none() {
            let state = self.menu_state();
            for command in menus::shortcuts(ctx, self.platform, &state) {
                self.run_command(ctx, command);
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, eframe_frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let frame = ctx.cumulative_pass_nr();
        self.art.begin_frame(&ctx);
        self.track_focus();
        self.track_edit_target(&ctx);
        self.refresh_appearance(&ctx);
        self.follow_full_screen(&ctx);
        self.window_chrome(ui);
        self.place_traffic_lights(&ctx, eframe_frame);
        let menu_list = menus::menus(self.platform, &self.menu_state());
        // No menu bar in the window (Chrome's way): the title bar's menu button opens the menus;
        // on macOS they are also in the menu bar at the top of the screen.
        let entries = menus::button_entries(&menu_list);
        let chosen = self.menu_button.show(&ctx, &entries, &self.theme, self.platform);
        #[cfg(target_os = "macos")]
        if let Some(bar) = &mut self.native_menu {
            bar.sync(&crate::native_menu::spec(&menu_list));
        }
        if let Some(command) = chosen {
            self.run_command(&ctx, command);
        }
        self.update_strip(ui);
        self.notice_strip(ui);
        self.official_map_strip(ui);
        egui::Panel::bottom("status-bar")
            .frame(
                egui::Frame::new()
                    .fill(self.chrome.footer.middle())
                    .stroke(egui::Stroke::new(1.0, self.theme.border))
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                shell::status_bar(ui, &self.sessions, self.active_session, &self.theme)
            });
        let visible = workspace::visible_sessions(&self.dock);
        let strips = workspace::document_leaves(&self.dock);
        let look = self.header_look();
        self.strips(ui);
        macro_rules! viewer {
            () => {
                Viewer {
                    sessions: &mut self.sessions,
                    actions: &mut self.actions,
                    theme: &self.theme,
                    fonts: &self.fonts,
                    fallback: &mut self.fallback,
                    settings: &mut self.settings,
                    frame,
                    art: &mut self.art,
                    directory: &mut self.dir_view,
                    directory_status: &self.directory_status,
                    directory_base: self.directory.base(),
                    panel: &mut self.panel,
                    channels: &mut self.channels,
                    maps: &mut self.maps,
                    full_maps: &mut self.full_maps,
                    split_share: self.split_share,
                    classification: self.classification.as_deref(),
                    active_session: self.active_session,
                    directory_active: self.directory_active,
                    visible: &visible,
                    now: unix_now() as i64,
                    stats: self.panel_stats.as_mut(),
                    heads: &self.heads,
                    strips: &strips,
                    look: &look,
                }
            };
        }
        let ground = self.chrome.ground;
        let header_height = self.chrome.skin.dock_header_height;
        // The centre documents have no tab bar, as in C# (the Workspace panel switches them).
        for (path, leaf) in self.dock.iter_leaves_mut() {
            leaf.tab_bar_hidden = path.surface.is_main() && leaf.tabs.iter().all(|t| t.is_document());
        }
        self.plan_headers(ui, &look);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ground).inner_margin(4))
            .show(ui, |ui| {
                self.dock_rect = ui.max_rect();
                let drop_target = self.drop.track(ui.ctx(), &self.dock, self.dock_rect, header_height);
                if let Some((tab, _)) = self.drop.dragging {
                    // egui_dock's floating window: at the pointer, 0.8 of the panel's size.
                    let floating = ui.ctx().input(|i| i.pointer.latest_pos()).and_then(|at| {
                        let path = self.dock.find_tab(&tab)?;
                        let size = self.dock.leaf(path.node_path()).ok()?.rect.size() * 0.8;
                        Some((egui::Rect::from_min_size(at, size), tab.label()))
                    });
                    crate::dock_drop::paint(
                        ui.ctx(),
                        &self.theme,
                        &self.dock,
                        self.dock_rect,
                        header_height,
                        drop_target,
                        floating,
                    );
                }
                // The shaded surface under tab bars the dock still draws (several panels in one
                // leaf), where each leaf's tab bar was last frame (the dock paints it transparent).
                for (path, leaf) in self.dock.iter_leaves() {
                    if leaf.rect.is_positive()
                        && !leaf.tab_bar_hidden
                        && !(path.surface.is_main() && leaf.tabs.len() == 1 && self.heads.contains_key(&leaf.tabs[0]))
                    {
                        let header =
                            egui::Rect::from_min_size(leaf.rect.min, egui::vec2(leaf.rect.width(), header_height));
                        skin::gradient_on(ui.painter(), header, &self.chrome.header);
                    }
                }
                let mut style = egui_dock::Style::from_egui(ui.style());
                style.tab_bar.bg_fill = egui::Color32::TRANSPARENT;
                style.tab_bar.height = header_height;
                style.tab_bar.hline_color = look.line.unwrap_or(self.chrome.rim_edge);
                style.separator.color_idle = ground;
                // A panel can be narrowed to 120 points (egui_dock's default stops at 175), so a
                // narrow column shows its actions dropping to a row as in C#.
                style.separator.extra = 120.0;
                style.tab.inactive.bg_fill = egui::Color32::TRANSPARENT;
                style.tab.tab_body.hidden_tab_bar_drag_height = Some(0.0);
                style.buttons.show_tab_bar_size = 0.0;
                style.buttons.show_tab_bar_hover_expand = 0.0;
                crate::dock_drop::style(&mut style, &self.theme);
                let separator = (style.separator.width, style.separator.extra_interact_width);
                let mut viewer = viewer!();
                DockArea::new(&mut self.dock)
                    .style(style)
                    .show_leaf_collapse_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .hidable_tab_bars(true)
                    .show_inside(ui, &mut viewer);
                name_dock(ui.ctx(), &self.dock, &mut viewer, separator);
                self.draw_headers(ui, &look);
                if self.drop.finish(&mut self.dock) {
                    self.save_layout(true);
                    ui.ctx().request_repaint();
                }
                if self.drop.pending.is_some() {
                    ui.ctx().request_repaint();
                }
            });
        if let Some(shown) = self.auto.shown {
            let mut viewer = viewer!();
            let overlay = overlay(&ctx, &self.auto, shown.tab, self.dock_rect, &self.theme, &mut viewer);
            self.track_overlay(&ctx, overlay);
        }
        self.show_toast(&ctx);
        if let Some(dialog) = &mut self.settings_dialog {
            match dialog.show(&ctx, &self.theme, self.directory.base()) {
                // Colour and skin changes preview while the dialog is open (as in C#): the
                // appearance follows the draft until Save or Cancel (refresh_appearance).
                SettingsResult::Open => {}
                SettingsResult::Cancelled => {
                    self.settings_dialog = None;
                    self.refresh_appearance(&ctx);
                }
                SettingsResult::ResetLayout => self.reset_layout(),
                SettingsResult::Saved(settings) => {
                    self.settings_dialog = None;
                    self.save_preferences_from_dialog(&ctx, *settings);
                }
            }
        }
        self.show_csharp_import(&ctx);
        self.show_map_import(&ctx);
        if let Some(dialog) = &self.dialog {
            match dialog.show(&ctx, &self.theme) {
                DialogResult::Open => {}
                DialogResult::Closed => self.dialog = None,
                DialogResult::OpenLink(url) => {
                    self.dialog = None;
                    self.open_link(&url);
                }
                DialogResult::MudletPick(pick) => {
                    self.dialog = None;
                    if let Some(path) = pick_mudlet_source(pick) {
                        self.import_mudlet(&path);
                    }
                }
            }
        }
        if let Some(form) = &mut self.form {
            form.lua_enabled = self.settings.enable_lua_scripts;
            form.global_command_style = self.settings.command_style;
            match form.show(&ctx, &self.theme, &self.settings.worlds) {
                FormResult::Open => {}
                FormResult::Cancelled => self.form = None,
                FormResult::Saved {
                    index,
                    world,
                    library,
                    password,
                    remember,
                } => match self.save_world_with_login(index, world, &password, remember) {
                    Ok(index) => {
                        if let Some(change) = library {
                            let world_id = self.settings.worlds[index].world_id.clone();
                            self.save_library(&world_id, change);
                        }
                        #[cfg(feature = "agent")]
                        let agent = self.save_agent_settings(index);
                        #[cfg(not(feature = "agent"))]
                        let agent: Result<(), String> = Ok(());
                        match agent {
                            Ok(()) => self.form = None,
                            // The world is saved; the agent settings stay open with the error
                            // (C# `SaveDraftAsync` throws after the world was written).
                            Err(e) => {
                                if let Some(form) = &mut self.form {
                                    if form.index.is_none() {
                                        form.index = Some(index);
                                    }
                                    form.agent_failed(e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(form) = &mut self.form {
                            form.login_failed(e);
                        }
                    }
                },
                FormResult::Removed(index) => {
                    self.form = None;
                    self.delete_world(index);
                }
                FormResult::Switch(index) => self.edit_world(index),
            }
        }
        if let Some(mark) = &mut self.mark {
            match mark.show(&ctx, &self.theme) {
                crate::mark_channel::MarkResult::Open => {}
                crate::mark_channel::MarkResult::Cancelled => self.mark = None,
                crate::mark_channel::MarkResult::Teach(id, rule) => match self.teach_channel_rule(id, rule) {
                    Ok(()) => self.mark = None,
                    Err(e) => {
                        if let Some(mark) = &mut self.mark {
                            mark.failed(e);
                        }
                    }
                },
            }
        }
        let sessions = &mut self.sessions;
        if let Some(window) = &mut self.history_window
            && !window.show(&ctx, &self.theme, || history_flushes(sessions))
        {
            self.history_window = None;
        }
        // Recorder threads of ended connections: keep the ones still writing for exit.
        for entry in self.sessions.iter_mut() {
            self.history_drains.extend(entry.tab.take_history_drains());
        }
        self.history_drains.retain(|h| !h.is_finished());
        // The directory is fetched the first time it is shown (never at start otherwise).
        if std::mem::take(&mut self.dir_view.wants_fetch) && !self.requested_fetch {
            self.requested_fetch = true;
            self.directory.refresh(false);
        }
        self.art.end_frame();
        self.fallback.poll(&ctx, &self.fonts.regular);
        self.apply(&ctx);
        self.save_layout(false);
        self.frames_presented += 1;
        if let Some(probe) = &mut self.probe {
            probe.frame_end();
        }
    }

    fn on_exit(&mut self) {
        // A toast still shown can no longer be undone: a deleted world's password goes now.
        if let Some(toast) = self.toast.take() {
            self.finish_toast(toast);
        }
        // Write pending settings and the layout before the process ends.
        self.save_layout(true);
        if let Some(saver) = &mut self.layout_saver {
            saver.flush();
        }
        if let Some(saver) = &mut self.saver {
            saver.flush();
        }
        if let Some(writer) = &mut self.db_writer {
            writer.close();
        }
        // Maps are saved and written before the process ends.
        for entry in self.sessions.iter_mut() {
            entry.tab.save_map(true);
        }
        if let Some(worker) = &self.map_worker {
            worker.flush();
        }
        // An orderly close writes what history still holds (bounded, so exit never hangs).
        for entry in self.sessions.iter_mut() {
            entry.tab.end_history();
            self.history_drains.extend(entry.tab.take_history_drains());
        }
        wandur_core::history::recorder::join_all(std::mem::take(&mut self.history_drains), Duration::from_secs(3));
    }
}

/// The scripts of a library (not its macros), as a session runs them.
pub fn script_definitions(library: &[LibraryEntry]) -> Vec<wandur_core::scripting::session::ScriptDefinition> {
    library
        .iter()
        .filter(|e| !e.is_macro())
        .map(|e| wandur_core::scripting::session::ScriptDefinition {
            id: e.id.clone(),
            name: e.name.clone(),
            source: e.source.clone(),
            // Imported code that needs conversion never runs.
            enabled: e.enabled && !e.needs_conversion(),
            restricted_send: e.restricted_send(),
            runtime: e.runtime(),
        })
        .collect()
}

/// The room classifier service: a test's classifier, else (with the `classifier` feature) the
/// package installed in the data directory or named by `WANDUR_ROOM_MODEL_DIR`. Creating it
/// reads manifests only; nothing is verified or loaded until a session needs a room classified.
fn classification_service(
    options: &Options,
    data_dir: Option<&std::path::Path>,
) -> Option<Arc<wandur_core::classify::RoomClassificationService>> {
    use wandur_core::classify::{ClassificationState, RoomClassificationService};
    if let Some(classifier) = &options.room_classifier {
        return Some(Arc::new(RoomClassificationService::for_testing(Arc::clone(classifier))));
    }
    let package = std::env::var_os("WANDUR_ROOM_MODEL_DIR").map(PathBuf::from);
    let service = RoomClassificationService::new(data_dir.filter(|_| !options.ephemeral), package);
    (service.status().state != ClassificationState::Unavailable).then(|| Arc::new(service))
}

/// The side-by-side share to save: only one the person chose (not the default).
fn chosen_split(share: f32) -> Option<f32> {
    (share != crate::terminal_view::DEFAULT_SPLIT_SHARE).then_some(share)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App as _;
    use egui::{Event, PointerButton, RawInput, pos2, vec2};
    use wandur_core::directory::client::Fetched;
    use wandur_core::l10n::text_in;

    struct Offline;

    impl Fetcher for Offline {
        fn get(&self, _url: &str, _limit: u64) -> Result<Fetched, String> {
            Err("offline test".into())
        }
    }

    fn frame(app: &mut WandurApp, ctx: &egui::Context, events: Vec<Event>) {
        let mut eframe_frame = eframe::Frame::_new_kittest();
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx(), &mut eframe_frame);
            app.ui(ui, &mut eframe_frame);
        });
        out.textures_delta.clear();
    }

    fn click(pos: egui::Pos2) -> Vec<Vec<Event>> {
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        vec![vec![Event::PointerMoved(pos), button(true)], vec![button(false)]]
    }

    /// Edit from the mini map (the old map editor document's successor) shows the session's Map
    /// page with Edit on, the current room selected; a second Edit shows the same full map; no
    /// document tab is added; closing the session drops its full map. With a classifier, the
    /// demo's rooms get their terrain guessed off the UI thread while frames run.
    #[test]
    fn edit_from_the_mini_map_opens_the_map_page_in_edit_mode() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.superpowers/test-data")
            .join(format!("map-editor-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                demo: true,
                room_classifier: Some(Arc::new(crate::scene::KeywordClassifier)),
                ..offline_options(&dir)
            },
        );
        frame(&mut app, &ctx, vec![]);
        let id = app.sessions.iter().next().unwrap().tab.id;
        let deadline = Instant::now() + Duration::from_secs(10);
        let classified = |app: &WandurApp| {
            let tracker = app.sessions.get(id).unwrap().tab.map.tracker();
            tracker.room_count() > 0 && tracker.inference_candidates().all(|r| r.inferred_key.is_some())
        };
        while !classified(&app) && Instant::now() < deadline {
            frame(&mut app, &ctx, vec![]);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(classified(&app), "the demo's rooms were classified");
        let current = app
            .sessions
            .get(id)
            .unwrap()
            .tab
            .map
            .tracker()
            .current_id()
            .map(str::to_string);
        let tabs = app.dock.iter_all_tabs().count();
        assert!(app.full_maps.is_empty(), "no full map before it is shown");
        app.actions.push(AppAction::OpenMapEditor(id));
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.dock.iter_all_tabs().count(), tabs, "no document of its own");
        let view = &app.sessions.get(id).unwrap().view;
        assert_eq!(view.page, crate::terminal_view::Page::Map);
        assert!(view.map_rect.is_some() && view.strip_rows_drawn > 0);
        let full = app.full_maps.get(&id).unwrap();
        assert!(full.is_editor());
        assert!(
            full.editor
                .as_deref()
                .is_some_and(|e| e.open && e.rooms == current.iter().cloned().collect::<Vec<_>>())
        );
        assert_eq!(full.selected, current);
        assert!(full.shown > 0);
        assert_eq!(app.active_session, Some(id));
        // Editing again shows the same full map.
        app.actions.push(AppAction::OpenMapEditor(id));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.full_maps.len(), 1);
        assert_eq!(app.dock.iter_all_tabs().count(), tabs);
        assert!(app.full_maps.get(&id).unwrap().is_editor());
        app.actions.push(AppAction::Close(id));
        frame(&mut app, &ctx, vec![]);
        assert!(app.dock.find_tab(&Tab::Session(id)).is_none());
        assert!(app.full_maps.is_empty());
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Play and Map switch per session (Cmd+Shift+M, Ctrl+Shift+M elsewhere), side by side shows
    /// both, a hidden full map is not drawn, and Escape on the map goes back to Play's composer.
    #[test]
    fn play_and_map_switch_per_session_and_side_by_side_shows_both() {
        use crate::terminal_view::Page;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.superpowers/test-data")
            .join(format!("play-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                demo: true,
                ..offline_options(&dir)
            },
        );
        frame(&mut app, &ctx, vec![]);
        app.actions.push(AppAction::OpenDemo);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let ids: Vec<SessionId> = app.sessions.iter().map(|e| e.tab.id).collect();
        assert_eq!(ids.len(), 2);
        let (first, second) = (ids[0], ids[1]);
        assert_eq!(app.active_session, Some(second));
        let page = |app: &WandurApp, id| app.sessions.get(id).unwrap().view.page;
        // The shortcut shows the active session's map; the other session stays on Play.
        let modifiers = if cfg!(target_os = "macos") {
            egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT
        };
        let key = |key: egui::Key, modifiers: egui::Modifiers| {
            vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]
        };
        frame(&mut app, &ctx, key(egui::Key::M, modifiers));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(page(&app, second), Page::Map);
        assert_eq!(page(&app, first), Page::Play);
        assert!(app.full_maps.contains_key(&second) && !app.full_maps.contains_key(&first));
        let view = &app.sessions.get(second).unwrap().view;
        assert!(view.map_rect.is_some());
        assert_eq!(view.strip_rows_drawn, crate::terminal_view::DEFAULT_STRIP_ROWS);
        assert_eq!(view.rows_drawn, 0, "the transcript is not drawn behind the map");
        // Side by side: the transcript and the map both drawn, no output strip.
        app.toggle_session_split();
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let view = &app.sessions.get(second).unwrap().view;
        assert!(view.split);
        let map = view.map_rect.unwrap();
        let transcript = view.transcript_rect.unwrap();
        assert!(
            transcript.right() <= map.left() && view.rows_drawn > 0,
            "{transcript:?} {map:?}"
        );
        assert_eq!(view.strip_rows_drawn, 0);
        assert!(app.menu_state().session_split);
        // Back to tabs: the page shown before (Map); then Play, and the full map is not drawn.
        app.toggle_session_split();
        frame(&mut app, &ctx, vec![]);
        assert_eq!(page(&app, second), Page::Map);
        assert!(app.menu_state().session_map);
        frame(&mut app, &ctx, key(egui::Key::M, modifiers));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(page(&app, second), Page::Play);
        let shown = app.full_maps.get(&second).unwrap().shown;
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.full_maps.get(&second).unwrap().shown, shown, "hidden, not drawn");
        assert!(app.sessions.get(second).unwrap().view.map_rect.is_none());
        // Escape on the map, with nothing focused, goes back to Play's composer.
        app.toggle_session_map();
        frame(&mut app, &ctx, vec![]);
        ctx.memory_mut(|m| {
            if let Some(f) = m.focused() {
                m.surrender_focus(f);
            }
        });
        frame(&mut app, &ctx, key(egui::Key::Escape, egui::Modifiers::NONE));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(page(&app, second), Page::Play);
        assert!(ctx.memory(|m| m.has_focus(egui::Id::new(("wandur-input", second)))));
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn jedi_theme() -> WorldTheme {
        let value: serde_json::Value = serde_json::from_str(crate::scene::WORLD_THEME_FIXTURE).unwrap();
        WorldTheme::parse(&value).unwrap()
    }

    /// A skin switch (View > Skin, the title bar's Skin menu) keeps every session, the dock and
    /// the transcripts as they are, and is saved; an unknown skin in the settings is Fleet.
    #[test]
    fn a_skin_switch_keeps_sessions_and_dock_content() {
        let dir = superpowers_dir("app-skin");
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                demo: true,
                ..offline_options(&dir)
            },
        );
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        let tabs: Vec<Tab> = app.dock.iter_all_tabs().map(|(_, t)| *t).collect();
        let sessions: Vec<SessionId> = app.sessions.iter().map(|e| e.tab.id).collect();
        let active = app.active_session;
        let text = app.sessions.iter().next().unwrap().tab.terminal.transcript();
        assert!(!text.is_empty());
        assert_eq!(app.chrome.skin, WindowSkin::FLEET);
        let check = |app: &WandurApp, skin: SkinId| {
            assert_eq!(app.settings.skin, skin.name());
            assert_eq!(app.skin, skin);
            assert_eq!(app.chrome.skin, WindowSkin::of(skin));
            assert_eq!(app.dock.iter_all_tabs().map(|(_, t)| *t).collect::<Vec<_>>(), tabs);
            assert_eq!(app.sessions.iter().map(|e| e.tab.id).collect::<Vec<_>>(), sessions);
            assert_eq!(app.active_session, active);
            assert_eq!(app.sessions.iter().next().unwrap().tab.terminal.transcript(), text);
        };
        for (i, skin) in [(1u8, SkinId::Armored), (2, SkinId::System), (0, SkinId::Fleet)] {
            app.run_command(&ctx, Command::Skin(i));
            frame(&mut app, &ctx, vec![]);
            check(&app, skin);
            assert_eq!(app.menu_state().skin, i);
        }
        app.title_action(&ctx, TitleAction::Skin(SkinId::Armored));
        frame(&mut app, &ctx, vec![]);
        check(&app, SkinId::Armored);
        app.on_exit();
        drop(app);
        // Saved, and read back; a skin this client does not know is Fleet.
        let (saved, _) = Settings::load(&dir, wandur_term::MAX_SCROLLBACK);
        assert_eq!(saved.skin, "Armored");
        std::fs::write(dir.join("settings.json"), r#"{"skin": "Steampunk"}"#).unwrap();
        let app = WandurApp::new(&ctx, offline_options(&dir));
        assert_eq!(app.settings.skin, "Fleet");
        assert_eq!(app.skin, SkinId::Fleet);
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A world's theme applies while one of its sessions is the active one, follows the focus
    /// between sessions, stays off when Settings turns world themes off, and the personal theme
    /// comes back when its session closes (the C# SessionWorkspace.ApplyAppearance).
    #[test]
    fn a_world_theme_follows_the_active_session_and_reverts_on_close() {
        let dir = superpowers_dir("app-world-theme");
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                demo: true,
                ..offline_options(&dir)
            },
        );
        frame(&mut app, &ctx, vec![]);
        let personal = Theme::from_settings(&app.settings);
        assert_eq!(app.theme, personal);
        let first = app.active_session.unwrap();
        let jedi = jedi_theme();
        app.sessions.get_mut(first).unwrap().tab.world_theme = Some(jedi.clone());
        frame(&mut app, &ctx, vec![]);
        let world = Theme::resolve(&app.settings, Some(&jedi));
        assert_eq!(app.theme, world);
        assert_eq!(app.theme.panel, crate::theme::parse_hex("#112532").unwrap());
        assert_eq!(app.applied_world.as_ref(), Some(&jedi));
        assert_eq!(ctx.global_style().visuals.panel_fill, world.panel);
        // A second session without a theme: the personal theme while it is active.
        app.run_command(&ctx, Command::OpenDemo);
        frame(&mut app, &ctx, vec![]);
        let second = app.active_session.unwrap();
        assert_ne!(second, first);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.theme, personal);
        assert!(app.applied_world.is_none());
        app.actions.push(AppAction::Focus(first));
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.theme, world, "focus back on the world's session");
        // Settings > Appearance can turn world themes off (previewed while the dialog is open).
        app.settings_dialog = Some(SettingsDialog::new(&app.settings));
        app.settings_dialog.as_mut().unwrap().draft.use_world_themes = false;
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.theme, personal);
        app.settings_dialog = None;
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.theme, world);
        // Closing the world's session puts the personal theme back.
        app.actions.push(AppAction::Close(first));
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.sessions.get(first).is_none());
        assert_eq!(app.theme, personal);
        assert!(app.applied_world.is_none());
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The palette menu's choice is saved and stops following worlds; Follow MUD theme turns
    /// it back on.
    #[test]
    fn the_palette_menu_chooses_a_scheme_and_world_following() {
        let dir = superpowers_dir("app-palette");
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        frame(&mut app, &ctx, vec![]);
        app.title_action(&ctx, TitleAction::Theme("Slate".into()));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.settings.theme, "Slate");
        assert!(!app.settings.use_world_themes);
        assert_eq!(app.theme, Theme::preset("Slate"));
        app.title_action(&ctx, TitleAction::ToggleWorldThemes);
        assert!(app.settings.use_world_themes);
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Theme switch time: resolving a theme, its skin colours and the egui visuals, for every
    /// preset in every skin, stays far under the 10 ms the plan allows (C# targets a frame).
    #[test]
    fn a_theme_switch_takes_under_ten_milliseconds() {
        let ctx = egui::Context::default();
        let jedi = jedi_theme();
        let mut worst = Duration::ZERO;
        for skin in SkinId::ALL {
            for name in Theme::names() {
                for world in [None, Some(&jedi)] {
                    let settings = Settings {
                        theme: name.into(),
                        skin: skin.name().into(),
                        ..Settings::default()
                    };
                    let started = Instant::now();
                    let theme = Theme::resolve(&settings, world);
                    let chrome = Chrome::new(&theme, skin, world.and_then(|w| w.skin.as_ref()));
                    theme.apply(&ctx);
                    worst = worst.max(started.elapsed());
                    assert_eq!(chrome.skin, WindowSkin::of(skin));
                }
            }
        }
        assert!(worst < Duration::from_millis(10), "worst theme switch {worst:?}");
    }

    fn offline_options(dir: &std::path::Path) -> Options {
        Options {
            data_dir: Some(dir.to_path_buf()),
            directory_url: Some("http://127.0.0.1:9".into()),
            fetcher: Some(Arc::new(Offline)),
            // Tests never touch the system's credential store, nor the C# client's data.
            vault: Some(Arc::new(wandur_core::login::MemoryVault::new())),
            csharp_vault: Some(Arc::new(wandur_core::login::MemoryVault::new())),
            csharp_folder: Some(dir.join("no-csharp-data")),
            ..Default::default()
        }
    }

    /// File > Import from Wandur (C#) end to end over the synthetic C# fixture: the dialog looks
    /// in the folder and counts what it holds; Import runs on a worker thread; the app takes the
    /// worlds and preferences, the maps and libraries are in wandur.db, the saved password moved
    /// from the C# entry to this client's; a second import adds nothing.
    #[test]
    fn import_from_wandur_csharp_brings_the_data_over_in_the_app() {
        use wandur_core::import::csharp::{Kind, secrets::csharp_login_key};
        let dir = superpowers_dir("app-csharp-import");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wandur-core/tests/fixtures/csharp-data/main");
        let from = Arc::new(wandur_core::login::MemoryVault::new());
        from.write(
            &csharp_login_key(
                "11111111-2222-4333-8444-555555555501",
                "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01",
                "lantern.fixture.example",
                4000,
                false,
                "wayfarer",
            ),
            "fixture-secret",
        )
        .unwrap();
        let to = Arc::new(wandur_core::login::MemoryVault::new());
        // This client's preferences are already changed, so they stay (and the UI language,
        // which is process-wide, is not switched under the other tests).
        let own = Settings {
            theme: "Ember".into(),
            ..Settings::default()
        };
        own.save(&dir).unwrap();
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                vault: Some(Arc::clone(&to) as Arc<dyn PasswordVault>),
                csharp_vault: Some(Arc::clone(&from) as Arc<dyn PasswordVault>),
                csharp_folder: Some(fixture),
                ..offline_options(&dir)
            },
        );
        frame(&mut app, &ctx, vec![]);
        app.run_command(&ctx, Command::ImportCsharp);
        frames_until(&mut app, &ctx, |a| a.csharp_import.as_ref().is_some_and(|d| !d.busy()));
        app.start_csharp_import();
        frames_until(&mut app, &ctx, |a| {
            a.csharp_import.as_ref().is_some_and(|d| d.summary().is_some())
        });
        frame(&mut app, &ctx, vec![]);
        let report = app.csharp_import.as_ref().unwrap().summary().unwrap().clone().unwrap();
        assert_eq!(report.tally(Kind::Worlds).imported, 4);
        assert_eq!(report.tally(Kind::Passwords).imported, 1);
        assert_eq!(
            report.tally(Kind::Preferences).unchanged + report.tally(Kind::Preferences).imported,
            0
        );
        let names: Vec<&str> = app.settings.worlds.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Fixture Lantern Road",
                "Fixture Glass Harbor",
                "Fixture Moss Hollow",
                "Fixture Ash Keep"
            ]
        );
        assert_eq!(app.settings.theme, "Ember");
        assert_eq!(app.settings.language, "");
        let key = wandur_core::login::vault::key(&app.settings.worlds[0]).unwrap();
        assert_eq!(to.read(&key).unwrap().as_deref(), Some("fixture-secret"));
        let world_id = app.settings.worlds[0].world_id.clone();
        assert_eq!(app.library(&world_id).len(), 6);
        // Imported scripts stay editable: only the C# pack script is a pack script.
        assert!(app.library(&world_id).iter().all(|e| !e.is_pack()));
        let packs: Vec<String> = app
            .settings
            .worlds
            .iter()
            .map(|w| w.world_id.clone())
            .collect::<Vec<_>>()
            .iter()
            .flat_map(|id| app.library(id))
            .filter(|e| e.is_pack())
            .map(|e| e.name)
            .collect();
        assert_eq!(packs, ["Fixture pack script"]);
        // The settings are saved with the imported worlds.
        if let Some(saver) = &mut app.saver {
            saver.flush();
        }
        let (saved, _) = Settings::load(&dir, wandur_term::MAX_SCROLLBACK);
        assert_eq!(saved.worlds.len(), 4);
        // Done closes it; a second import adds nothing.
        app.csharp_import = None;
        app.run_command(&ctx, Command::ImportCsharp);
        frames_until(&mut app, &ctx, |a| a.csharp_import.as_ref().is_some_and(|d| !d.busy()));
        app.start_csharp_import();
        frames_until(&mut app, &ctx, |a| {
            a.csharp_import.as_ref().is_some_and(|d| d.summary().is_some())
        });
        frame(&mut app, &ctx, vec![]);
        let again = app.csharp_import.as_ref().unwrap().summary().unwrap().clone().unwrap();
        assert!(
            Kind::ALL.iter().all(|k| again.tally(*k).imported == 0),
            "{}",
            again.to_text()
        );
        assert_eq!(app.settings.worlds.len(), 4);
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// LoginTests.LoginFieldsSaveThroughDialog and ProfileEditKeepsPassword...: the Login
    /// section's fields save through the app into the vault, never into settings.json; editing
    /// the host without a new password is refused (the editor stays open with the error);
    /// renaming keeps the password; removing the world forgets it.
    #[test]
    fn login_fields_save_through_the_editor_into_the_vault_only() {
        let dir = superpowers_dir("app-login");
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                vault: Some(Arc::clone(&vault) as Arc<dyn PasswordVault>),
                ..offline_options(&dir)
            },
        );
        let mut form = WorldForm::new_world().with_vault_name(vault.name());
        form.world.name = "Second".into();
        form.world.host = "second.example.org".into();
        form.port_text = "4000".into();
        form.section = crate::world_form::Section::Login;
        form.world.username = " test-player ".into();
        form.set_remember(true);
        form.password = "dialog-secret".into();
        form.world.auto_login = true;
        let FormResult::Saved {
            index,
            world,
            password,
            remember,
            ..
        } = form.save()
        else {
            panic!("expected a save");
        };
        let i = app.save_world_with_login(index, world, &password, remember).unwrap();
        let saved = app.settings.worlds[i].clone();
        assert_eq!(saved.username, "test-player");
        assert!(saved.auto_login);
        let key = wandur_core::login::vault::key(&saved).unwrap();
        assert_eq!(vault.read(&key).unwrap().as_deref(), Some("dialog-secret"));

        // A new host needs the password again; the world is left as it was.
        let mut moved = saved.clone();
        moved.host = "other.example.org".into();
        let e = app.save_world_with_login(Some(i), moved, "", true).unwrap_err();
        assert_eq!(e, t(S::PasswordLoginChanged));
        assert_eq!(app.settings.worlds[i].host, "second.example.org");
        // Renaming keeps it.
        let mut renamed = saved.clone();
        renamed.name = "Renamed".into();
        app.save_world_with_login(Some(i), renamed, "", true).unwrap();
        assert_eq!(vault.read(&key).unwrap().as_deref(), Some("dialog-secret"));

        // The session opened from it shows the character and logs in with the saved password.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut local = app.settings.worlds[i].clone();
        local.host = "127.0.0.1".into();
        local.port = listener.local_addr().unwrap().port();
        app.save_world_with_login(Some(i), local, "dialog-secret", true)
            .unwrap();
        assert_eq!(vault.len(), 1, "the replaced entry was removed");
        app.open(app.settings.worlds[i].endpoint(), Some(i));
        let tab = &app.sessions.iter().next().unwrap().tab;
        assert_eq!(tab.title(), "Renamed · test-player");
        frame(&mut app, &ctx, vec![]);
        assert_eq!(
            app.title,
            format!("test-player · Renamed · {}", crate::dialogs::APP_NAME)
        );

        app.on_exit();
        let json = std::fs::read_to_string(dir.join(wandur_core::settings::SETTINGS_FILE)).unwrap();
        assert!(json.contains("test-player") && json.contains("password_id"));
        assert!(!json.contains("dialog-secret"), "{json}");

        app.delete_world(i);
        assert_eq!(vault.len(), 1, "kept while the undo toast can bring the world back");
        let toast = app.toast.take().unwrap();
        app.finish_toast(toast);
        assert!(
            vault.is_empty(),
            "removing the world forgot its password once the toast went"
        );
        drop(app);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// LoginTests.FailedSettingsSaveRemovesNewCredentialAndKeepsExistingCredential, through
    /// the app: settings.json cannot be replaced, so the new password is removed again and the
    /// saved one stays.
    #[test]
    fn a_failed_settings_write_removes_the_new_password() {
        let dir = superpowers_dir("app-login-fail");
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                vault: Some(Arc::clone(&vault) as Arc<dyn PasswordVault>),
                ..offline_options(&dir)
            },
        );
        let world = SavedWorld {
            name: "Example".into(),
            host: "example.org".into(),
            username: "player".into(),
            auto_login: true,
            ..SavedWorld::default()
        };
        let i = app.save_world_with_login(None, world, "original-secret", true).unwrap();
        let saved = app.settings.worlds[i].clone();
        let key = wandur_core::login::vault::key(&saved).unwrap();
        app.on_exit();
        let file = dir.join(wandur_core::settings::SETTINGS_FILE);
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir_all(&file).unwrap();
        assert!(
            app.save_world_with_login(Some(i), saved.clone(), "replacement-secret", true)
                .is_err()
        );
        assert_eq!(vault.len(), 1);
        assert_eq!(vault.read(&key).unwrap().as_deref(), Some("original-secret"));
        assert_eq!(app.settings.worlds[i], saved);
        std::fs::remove_dir_all(&file).unwrap();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A saved world gets a stable id; editing its host and port in the form keeps the id, and
    /// the database maps both addresses to it. The writes ran on the writer thread.
    #[test]
    fn world_ids_survive_address_edits_through_the_app() {
        let dir = std::env::temp_dir().join(format!("wandur-app-world-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        assert!(dir.join(wandur_core::db::DB_FILE).exists());
        let mut form = WorldForm::new_world();
        form.world.name = "The Lantern Road".into();
        form.world.host = "lanternroad.example.org".into();
        form.port_text = "4000".into();
        let world = form.finish().unwrap();
        app.save_world(None, world);
        let id = app.settings.worlds[0].world_id.clone();
        assert_eq!(id.len(), 32);
        let mut form = WorldForm::edit(Some(0), app.settings.worlds[0].clone());
        form.world.host = "127.0.0.1".into();
        form.port_text = "4513".into();
        let edited = form.finish().unwrap();
        app.save_world(Some(0), edited);
        assert_eq!(app.settings.worlds[0].world_id, id, "the edit kept the id");
        let writer = app.db_writer.as_ref().unwrap();
        writer.flush();
        let thread = writer.stats().thread.lock().unwrap().clone().unwrap();
        assert_ne!(thread, format!("{:?}", std::thread::current().id()));
        let db = app.db.clone().unwrap();
        let conn = db.connect().unwrap();
        assert_eq!(
            worlds::endpoints_of(&conn, &id).unwrap(),
            ["127.0.0.1:4513", "lanternroad.example.org:4000"]
        );
        // Restart: the id comes back from settings.json.
        app.on_exit();
        drop(app);
        let app = WandurApp::new(&ctx, offline_options(&dir));
        assert_eq!(app.settings.worlds[0].world_id, id);
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A throwaway data directory under the repository's `.superpowers/` (gitignored).
    fn superpowers_dir(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.superpowers/test-data")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Agent settings saved in the world editor go to `wandur.db` (the key to the vault), open
    /// sessions of that world take the new goals at once (their runs stop), and the Agent
    /// menu's Configure agent... opens the section (C# `AgentSectionAndLiveMenuBindToTheOriginatingWorld`).
    #[cfg(feature = "agent")]
    #[test]
    fn agent_settings_saved_in_the_world_editor_reach_the_database_vault_and_sessions() {
        use wandur_core::agent::{
            AgentProfileStore, AgentWorld, ProviderRegistry, SqliteAgentProfileStore, credentials,
        };
        let dir = superpowers_dir("app-agent");
        let ctx = egui::Context::default();
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        let mut options = offline_options(&dir);
        options.vault = Some(Arc::clone(&vault) as Arc<dyn PasswordVault>);
        // No model server: nothing is looked up.
        options.agent_providers = Some(ProviderRegistry::default());
        options.agent_no_discovery = true;
        let mut app = WandurApp::new(&ctx, options);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut form = WorldForm::new_world();
        form.world.name = "The Lantern Road".into();
        form.world.host = "127.0.0.1".into();
        form.port_text = port.to_string();
        let FormResult::Saved { index, world, .. } = form.save() else {
            panic!("expected a save");
        };
        let i = app.save_world(index, world);
        app.open(app.settings.worlds[i].endpoint(), Some(i));
        let (_server, _) = listener.accept().unwrap();
        let session = app.sessions.iter().next().unwrap().tab.id;
        assert!(app.sessions.get(session).unwrap().tab.agent.goals.is_empty());
        // Configure agent... opens the world editor on Agent settings.
        app.actions.push(AppAction::EditAgent(session));
        frame(&mut app, &ctx, vec![]);
        let form = app.form.as_mut().expect("the world editor is open");
        assert_eq!(form.section, crate::world_form::Section::Agent);
        let draft = form.agent.as_mut().expect("the agent draft");
        assert_eq!(draft.endpoint, "http://localhost:1234/v1");
        draft.model = "gemma-12b".into();
        draft.add_template("explore");
        draft.set_default(0, true);
        draft.set_api_key("token-secret", Instant::now());
        assert!(form.has_unsaved_changes());
        // Save world: the form checks the draft, the app saves the world, then the agent.
        let FormResult::Saved {
            index,
            world,
            password,
            remember,
            ..
        } = form.save()
        else {
            panic!("expected a save");
        };
        let i = app.save_world_with_login(index, world, &password, remember).unwrap();
        app.save_agent_settings(i).unwrap();
        let world_id = app.settings.worlds[i].world_id.clone();
        let store = SqliteAgentProfileStore::new(app.db.clone().unwrap());
        let saved = store.load(&AgentWorld::Id(world_id)).unwrap();
        assert_eq!(saved.model, "gemma-12b");
        assert_eq!(saved.goals.len(), 1);
        assert!(saved.goals[0].enabled);
        assert_eq!(
            credentials::read(vault.as_ref(), &saved).unwrap().as_deref(),
            Some("token-secret")
        );
        assert!(
            !std::fs::read_to_string(dir.join("settings.json"))
                .unwrap_or_default()
                .contains("token-secret")
        );
        let tab = &app.sessions.get(session).unwrap().tab;
        assert_eq!(tab.agent.goals.len(), 1, "the open session took the new goals");
        assert!(tab.agent.has_goal());
        // An invalid draft is refused before anything is written.
        app.edit_world(Some(i));
        let form = app.form.as_mut().unwrap();
        form.agent.as_mut().unwrap().commands = "look | look;quit | Chain".into();
        assert!(matches!(form.save(), FormResult::Open));
        assert_eq!(form.section, crate::world_form::Section::Agent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Scripts saved in the world editor reach a session opened afterwards and run; a later save
    /// leaves the open session's scripts alone until Scripts > Reload saved rules (C#); the
    /// menu's Edit configuration opens the Scripts section; an empty library gets the starter.
    #[test]
    fn scripts_saved_in_the_world_editor_run_and_reload_in_open_sessions() {
        use std::io::{Read, Write};
        let dir = superpowers_dir("app-scripts");
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut form = WorldForm::new_world().with_library(Vec::new());
        form.world.name = "The Lantern Road".into();
        form.world.host = "127.0.0.1".into();
        form.port_text = port.to_string();
        form.macros.add_script().unwrap();
        {
            let entry = form.macros.selected_script_mut().unwrap();
            assert_eq!(entry.name, t(S::ScriptDefaultName));
            assert!(!entry.enabled, "new scripts start disabled");
            entry.name = "Greeter".into();
            entry.source = "mud.trigger(/^Wren arrives/, () => mud.send('wave Wren'));".into();
            entry.enabled = true;
        }
        let FormResult::Saved {
            index,
            world,
            library: Some(change),
            ..
        } = form.save()
        else {
            panic!("expected a save with scripts");
        };
        let i = app.save_world(index, world);
        let id = app.settings.worlds[i].world_id.clone();
        app.save_library(&id, change.clone());
        app.db_writer.as_ref().unwrap().flush();
        app.open(app.settings.worlds[i].endpoint(), Some(i));
        let (mut server, _) = listener.accept().unwrap();
        let session = app.sessions.iter().next().unwrap().tab.id;
        let pump = |app: &mut WandurApp, done: &dyn Fn(&SessionTab) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let tab = &mut app.sessions.get_mut(session).unwrap().tab;
                tab.pump(Instant::now());
                if done(tab) {
                    break;
                }
                assert!(Instant::now() < deadline, "timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        pump(&mut app, &|t| t.scripts.entries.first().is_some_and(|e| e.is_running()));
        let read = |server: &mut std::net::TcpStream, app: &mut WandurApp, want: &str| {
            server.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut got = Vec::new();
            let mut buf = [0u8; 256];
            while !String::from_utf8_lossy(&got).contains(want) {
                assert!(Instant::now() < deadline, "no {want}");
                app.sessions.get_mut(session).unwrap().tab.pump(Instant::now());
                if let Ok(n) = server.read(&mut buf) {
                    got.extend_from_slice(&buf[..n]);
                }
            }
        };
        server.write_all(b"Wren arrives from the east.\r\n").unwrap();
        read(&mut server, &mut app, "wave Wren");

        // A new save: the open session keeps what it loaded until Reload saved rules.
        let mut edited = change.after.clone();
        edited[0].source = "mud.trigger(/^Wren arrives/, () => mud.send('bow Wren'));".into();
        app.save_library(
            &id,
            LibraryChange {
                before: change.after.clone(),
                after: edited,
            },
        );
        app.db_writer.as_ref().unwrap().flush();
        assert!(
            app.sessions.get(session).unwrap().tab.scripts.entries[0]
                .source
                .contains("wave")
        );
        app.actions.push(AppAction::ReloadScripts(session));
        frame(&mut app, &ctx, vec![]);
        assert!(
            app.sessions.get(session).unwrap().tab.scripts.entries[0]
                .source
                .contains("bow")
        );
        pump(&mut app, &|t| t.scripts.entries[0].is_running());
        server.write_all(b"Wren arrives again.\r\n").unwrap();
        read(&mut server, &mut app, "bow Wren");

        // The menu's Edit configuration and Session > Scripts open the Scripts section.
        app.actions.push(AppAction::EditScripts(session));
        frame(&mut app, &ctx, vec![]);
        let form = app.form.as_ref().expect("the world editor is open");
        assert_eq!(form.section, crate::world_form::Section::Scripts);
        assert_eq!(
            form.macros.selected_script_entry().map(|e| e.name.as_str()),
            Some("Greeter")
        );
        app.form = None;
        assert!(app.menu_state().has_scripts);

        // A world whose library was never opened gets the starter script, once.
        let fresh = wandur_core::db::worlds::new_world_id();
        let library = app.library(&fresh);
        assert_eq!(library.len(), 1);
        assert_eq!(library[0].source, t(S::ScriptStarterExample));
        app.db_writer.as_ref().unwrap().flush();
        app.libraries.clear();
        assert_eq!(app.library(&fresh), library, "stored once, not added again");
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// File > Import from Mudlet end to end (C# `MudletImportLibraryTests` and
    /// `LuaScriptLibraryTests`): the chooser names the selected world; a profile file adds its
    /// world and library and shows the summary; the converted JavaScript runs in a session;
    /// a Mudlet-compatible Lua script runs only once Allow Lua scripts is on.
    #[test]
    fn a_mudlet_import_adds_the_world_and_lua_runs_once_allowed() {
        use std::io::{Read, Write};
        use wandur_core::scripting::{Compatibility, Language};
        let dir = superpowers_dir("app-mudlet");
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../wandur-core/tests/fixtures/mudlet/lantern-road-profile.xml"),
        )
        .unwrap()
        .replace("<url>lanternroad.example.net</url>", "<url>127.0.0.1</url>")
        .replace("<port>4100</port>", &format!("<port>{port}</port>"))
        .replace("mSslTsl=\"yes\"", "mSslTsl=\"no\"");
        let path = dir.join("lantern-road.xml");
        std::fs::write(&path, fixture).unwrap();

        app.run_command(&ctx, Command::ImportMudlet);
        assert_eq!(app.dialog, Some(Dialog::MudletChooser { target: None }));
        app.dialog = None;
        app.import_mudlet(&path);
        let Some(Dialog::MudletSummary(text)) = app.dialog.clone() else {
            panic!("no summary: {:?}", app.notes);
        };
        assert!(text.starts_with("Added the world The Lantern Road."), "{text}");
        let i = app
            .settings
            .worlds
            .iter()
            .position(|w| w.port == port)
            .expect("the world is added");
        let world_id = app.settings.worlds[i].world_id.clone();
        app.db_writer.as_ref().unwrap().flush();
        app.libraries.clear();
        let library = app.library(&world_id);
        assert!(library.iter().any(|e| e.name == "Travel (Mudlet)" && e.enabled));
        assert!(library.iter().any(|e| e.needs_conversion() && !e.enabled));
        // A second import of the same profile changes nothing and adds no world.
        app.dialog = None;
        app.import_mudlet(&path);
        assert!(text.contains("Working now"));
        assert_eq!(app.settings.worlds.iter().filter(|w| w.port == port).count(), 1);
        app.db_writer.as_ref().unwrap().flush();
        app.libraries.clear();
        assert_eq!(app.library(&world_id), library);

        // A Mudlet-compatible Lua script beside them.
        let mut after = library.clone();
        after.push(LibraryEntry {
            id: scripts::new_id(),
            name: "Lantern (Lua)".into(),
            source: "tempTrigger('lantern', function() send('warm hands with Lua') end)".into(),
            enabled: true,
            language: Language::Lua,
            compatibility: Compatibility::Mudlet,
            ..LibraryEntry::default()
        });
        app.save_library(&world_id, LibraryChange { before: library, after });
        app.db_writer.as_ref().unwrap().flush();
        app.open(app.settings.worlds[i].endpoint(), Some(i));
        let (mut server, _) = listener.accept().unwrap();
        let session = app.sessions.iter().next().unwrap().tab.id;
        let read = |server: &mut std::net::TcpStream, app: &mut WandurApp, want: &str| {
            server.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut got = Vec::new();
            let mut buf = [0u8; 256];
            while !String::from_utf8_lossy(&got).contains(want) {
                assert!(
                    Instant::now() < deadline,
                    "no {want}: {}",
                    String::from_utf8_lossy(&got)
                );
                app.sessions.get_mut(session).unwrap().tab.pump(Instant::now());
                if let Ok(n) = server.read(&mut buf) {
                    got.extend_from_slice(&buf[..n]);
                }
            }
            String::from_utf8_lossy(&got).into_owned()
        };
        let lua_entry = |app: &WandurApp| {
            app.sessions
                .get(session)
                .unwrap()
                .tab
                .scripts
                .entries
                .iter()
                .find(|e| e.name == "Lantern (Lua)")
                .cloned()
                .unwrap()
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app
            .sessions
            .get(session)
            .unwrap()
            .tab
            .scripts
            .entries
            .iter()
            .any(|e| e.name == "Travel (Mudlet)" && e.is_running())
        {
            assert!(Instant::now() < deadline, "the converted script never ran");
            app.sessions.get_mut(session).unwrap().tab.pump(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
        // The converted JavaScript answers the tollkeeper; Lua is still off.
        server.write_all(b"The tollkeeper waits.\r\n").unwrap();
        read(&mut server, &mut app, "pay toll");
        assert!(!lua_entry(&app).is_running(), "Lua scripts are off by default");

        let mut settings = app.settings.clone();
        settings.enable_lua_scripts = true;
        app.save_preferences(&ctx, settings);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !lua_entry(&app).is_running() {
            assert!(
                Instant::now() < deadline,
                "the Lua script never ran: {:?}",
                lua_entry(&app).error
            );
            app.sessions.get_mut(session).unwrap().tab.pump(Instant::now());
            std::thread::sleep(Duration::from_millis(5));
        }
        server.write_all(b"A lantern flickers.\r\n").unwrap();
        read(&mut server, &mut app, "warm hands with Lua");
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Macros saved in the world editor go to wandur.db on the writer thread, reach a session
    /// opened afterwards, and are read back by the next run.
    #[test]
    fn macros_saved_in_the_world_editor_reach_the_database_and_sessions() {
        use wandur_core::macros::MacroKind;
        let dir = superpowers_dir("app-macros");
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut form = WorldForm::new_world().with_library(Vec::new());
        form.world.name = "The Lantern Road".into();
        form.world.host = "127.0.0.1".into();
        form.port_text = port.to_string();
        form.macros.add().unwrap();
        form.macros.set_kind(MacroKind::Alias);
        {
            let def = form.macros.definition_mut().unwrap();
            def.pattern = "ford".into();
            def.commands = "east\neast".into();
        }
        let FormResult::Saved {
            index,
            world,
            library: Some(change),
            ..
        } = form.save()
        else {
            panic!("expected a save with macros");
        };
        let i = app.save_world(index, world);
        let id = app.settings.worlds[i].world_id.clone();
        app.save_library(&id, change);
        app.db_writer.as_ref().unwrap().flush();
        let db = app.db.clone().unwrap();
        let stored = db.read(|c| scripts::load(c, &id)).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].macro_def.as_ref().unwrap().pattern, "ford");
        assert!(!stored[0].enabled, "new macros start disabled");

        app.open(app.settings.worlds[i].endpoint(), Some(i));
        let tab = &app.sessions.iter().next().unwrap().tab;
        assert_eq!(tab.world_id.as_deref(), Some(id.as_str()));
        assert!(tab.macros.has_macros());
        app.on_exit();
        drop(app);

        let mut again = WandurApp::new(&ctx, offline_options(&dir));
        assert_eq!(again.library(&id), stored);
        again.on_exit();
        drop(again);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pump the app until `done` (a loopback session is talking).
    fn frames_until(app: &mut WandurApp, ctx: &egui::Context, done: impl Fn(&WandurApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(app) {
            assert!(Instant::now() < deadline, "timed out");
            frame(app, ctx, vec![]);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The command style tip through the app, over loopback: Enter on `#2 look` while the style
    /// uses `/` shows the tip and sends nothing; Escape puts it away and keeps the line; Use #
    /// makes TinTin++ the global style and runs the line under it; Keep / sends the line as typed
    /// and is remembered, in settings.json too. Escape also stops a running line.
    #[test]
    fn the_command_style_tip_switches_or_keeps_and_is_remembered() {
        use crate::session_tab::test_support::read_lines;
        let dir = superpowers_dir("app-style-tip");
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                ..offline_options(&dir)
            },
        );
        assert_eq!(app.settings.command_style, CommandStyle::Wandur, "the default");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let i = app.save_world(
            None,
            SavedWorld {
                name: "Realm".into(),
                host: "127.0.0.1".into(),
                port,
                ..SavedWorld::default()
            },
        );
        app.open(app.settings.worlds[i].endpoint(), Some(i));
        let (mut server, _) = listener.accept().unwrap();
        let id = app.active_session.unwrap();
        frames_until(&mut app, &ctx, |a| {
            a.sessions.get(id).is_some_and(|e| e.tab.is_connected())
        });
        fn tab(app: &mut WandurApp) -> &mut crate::session_tab::SessionTab {
            let id = app.active_session.unwrap();
            &mut app.sessions.get_mut(id).unwrap().tab
        }
        // The command box has the keyboard (asked once: asking again resets what it keeps).
        tab(&mut app).focus_input = true;
        frame(&mut app, &ctx, vec![]);
        let press = |app: &mut WandurApp, key| {
            frame(app, &ctx, vec![key_event(key, true, Default::default())]);
            frame(app, &ctx, vec![key_event(key, false, Default::default())]);
        };

        tab(&mut app).input = "#2 look".into();
        press(&mut app, egui::Key::Enter);
        assert!(tab(&mut app).style_tip_shown(), "Enter asks");
        assert_eq!(tab(&mut app).input, "#2 look");
        assert!(tab(&mut app).history().is_empty(), "nothing went");
        press(&mut app, egui::Key::Escape);
        assert!(!tab(&mut app).style_tip_shown(), "Escape puts it away");
        assert_eq!(tab(&mut app).input, "#2 look", "and keeps the line");

        // Use #: TinTin++ becomes the global style and the line runs under it.
        press(&mut app, egui::Key::Enter);
        assert!(tab(&mut app).style_tip_shown());
        app.actions.push(AppAction::CommandStyleTip(id, true));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.settings.command_style, CommandStyle::TinTin);
        assert!(app.settings.command_style_tip_answered);
        assert!(!tab(&mut app).style_tip_shown());
        assert_eq!(tab(&mut app).command_style(), CommandStyle::TinTin);
        assert_eq!(read_lines(&mut server, 1), ["look"]);
        frames_until(&mut app, &ctx, |a| {
            a.sessions.get(id).is_some_and(|e| e.tab.queue_progress().is_none())
        });
        assert_eq!(read_lines(&mut server, 1), ["look"]);
        assert!(tab(&mut app).input.is_empty());

        // Keep /: the line goes as typed, the style stays, and it never asks again.
        app.settings.command_style = CommandStyle::Wandur;
        app.settings.command_style_tip_answered = false;
        app.actions.push(AppAction::SettingsChanged);
        frame(&mut app, &ctx, vec![]);
        tab(&mut app).input = "#3 n".into();
        press(&mut app, egui::Key::Enter);
        assert!(tab(&mut app).style_tip_shown());
        app.actions.push(AppAction::CommandStyleTip(id, false));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(read_lines(&mut server, 1), ["#3 n"]);
        assert_eq!(app.settings.command_style, CommandStyle::Wandur);
        assert!(app.settings.command_style_tip_answered);
        tab(&mut app).input = "#wait 2".into();
        press(&mut app, egui::Key::Enter);
        assert!(!tab(&mut app).style_tip_shown(), "answered: it never asks again");
        assert_eq!(read_lines(&mut server, 1), ["#wait 2"]);

        // Escape stops a running line: the command box keeps the keyboard for it.
        tab(&mut app).input = "/50 n".into();
        press(&mut app, egui::Key::Enter);
        assert!(tab(&mut app).queue_progress().is_some());
        press(&mut app, egui::Key::Escape);
        assert_eq!(tab(&mut app).queue_progress(), None);
        assert!(tab(&mut app).terminal.transcript().contains(" of 50 sent]"));

        app.on_exit();
        drop(app);
        let (saved, _) = Settings::load(&dir, wandur_term::MAX_SCROLLBACK);
        assert_eq!(saved.command_style, CommandStyle::Wandur);
        assert!(saved.command_style_tip_answered);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// TeachChannelTests through the app, against a loopback SMAUG world: a right click on a
    /// transcript line opens the menu for that line; Mark as channel proposes a rule and
    /// previews it over the transcript; the saved rule is on the world, in settings.json, and in
    /// force for the very next line beside the family's; Not a channel teaches an exclusion that
    /// beats the family; a rule edited in the world editor reaches the open session; the demo
    /// cannot be taught.
    #[test]
    fn a_taught_rule_applies_at_once_and_not_a_channel_beats_the_family() {
        use std::io::Write as _;
        use wandur_core::channels::ChannelRule;
        let dir = superpowers_dir("app-teach-channel");
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                ..offline_options(&dir)
            },
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let i = app.save_world(
            None,
            SavedWorld {
                name: "Realm".into(),
                host: "127.0.0.1".into(),
                port,
                codebase: "SMAUG 1.4a".into(),
                ..SavedWorld::default()
            },
        );
        app.open(app.settings.worlds[i].endpoint(), Some(i));
        let (mut server, _) = listener.accept().unwrap();
        let id = app.active_session.unwrap();
        server
            .write_all(b"You are standing in a wide green field.\r\n[CLAN] Vex: meeting at dawn\r\n[CLAN] Talon: bring the ship\r\nA small bird lands nearby.\r\n[CLAN] Vex: and the crew\r\n")
            .unwrap();
        let text = |app: &WandurApp, needle: &str| {
            app.sessions
                .get(id)
                .is_some_and(|e| e.tab.terminal.transcript().contains(needle))
        };
        frames_until(&mut app, &ctx, |a| text(a, "and the crew"));
        frame(&mut app, &ctx, vec![]);

        // A right click on the second row: no selection, the menu is for that line.
        let view = &app.sessions.get(id).unwrap().view;
        let rect = view.transcript_rect.expect("the transcript is drawn");
        let at = rect.min + vec2(30.0, view.cell.y * 1.5);
        let right = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, &ctx, vec![Event::PointerMoved(at), right(true)]);
        frame(&mut app, &ctx, vec![right(false)]);
        frame(&mut app, &ctx, vec![]);
        let target = app
            .sessions
            .get(id)
            .unwrap()
            .view
            .menu
            .clone()
            .expect("the menu opened");
        assert_eq!(target.examples(), ("[CLAN] Vex: meeting at dawn".to_string(), None));
        assert!(target.selection.is_none());

        let (example, second) = target.examples();
        app.actions.push(AppAction::MarkChannel(id, example, second));
        frame(&mut app, &ctx, vec![]);
        assert!(app.mark.is_some());
        frame(
            &mut app,
            &ctx,
            vec![Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert!(app.mark.is_none(), "Escape cancels");
        app.actions
            .push(AppAction::MarkChannel(id, "[CLAN] Vex: meeting at dawn".into(), None));
        frame(&mut app, &ctx, vec![]);
        let mark = app.mark.as_mut().expect("the dialog is open");
        assert!(mark.can_teach);
        assert!(mark.is_new_channel());
        assert_eq!(mark.channel(), "clan");
        assert_eq!(mark.match_count, 3);
        assert!(mark.line_count() >= 5);
        let rule = mark.save().unwrap();
        app.teach_channel_rule(id, rule.clone()).unwrap();
        app.mark = None;
        assert_eq!(app.settings.worlds[i].channel_rules, std::slice::from_ref(&rule));
        assert_eq!(
            rule,
            ChannelRule::new("clan", r"^\[CLAN\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$", Some("clan"))
        );
        assert!(app.notes.iter().any(|n| n.contains("Realm")), "{:?}", app.notes);
        {
            let tab = &app.sessions.get(id).unwrap().tab;
            assert_eq!(tab.channels.rules().rules()[0].channel, "clan");
            assert!(tab.channels.rules().channels().contains(&"clan".to_string()));
        }
        server
            .write_all(b"[CLAN] Talon: docking now\r\n[OOC] Aldric: still here\r\n")
            .unwrap();
        frames_until(&mut app, &ctx, |a| text(a, "still here"));
        {
            let log = &app.sessions.get(id).unwrap().tab.channels.log;
            let clan = log.tabs().iter().find(|t| t.channel == "clan").expect("a clan tab");
            assert_eq!(clan.messages.len(), 1);
            assert_eq!(clan.messages[0].speaker, "Talon");
            assert_eq!(clan.messages[0].text, "docking now");
            assert_eq!(clan.messages[0].reply_command.as_deref(), Some("clan"));
            // The family still does its part beside the taught rule.
            assert_eq!(
                log.tabs().iter().find(|t| t.channel == "ooc").unwrap().messages.len(),
                1
            );
        }

        // Not a channel: an exclusion for the same shape beats the family's ooc rule.
        app.actions
            .push(AppAction::MarkChannel(id, "[OOC] Aldric: still here".into(), None));
        frame(&mut app, &ctx, vec![]);
        let mark = app.mark.as_mut().unwrap();
        assert_eq!(mark.current_channel.as_deref(), Some("ooc"));
        assert!(mark.can_exclude());
        let exclusion = mark.exclude().unwrap();
        app.teach_channel_rule(id, exclusion.clone()).unwrap();
        app.mark = None;
        assert!(app.settings.worlds[i].channel_rules[1].exclude);
        server
            .write_all(b"[OOC] Brenna: welcome back\r\n[CHAT] Eowyn: who wants to group up?\r\n")
            .unwrap();
        frames_until(&mut app, &ctx, |a| text(a, "group up?"));
        {
            let log = &app.sessions.get(id).unwrap().tab.channels.log;
            assert_eq!(
                log.tabs().iter().find(|t| t.channel == "ooc").unwrap().messages.len(),
                1
            );
            assert_eq!(
                log.tabs().iter().find(|t| t.channel == "chat").unwrap().messages.len(),
                1
            );
        }

        // The world editor turns the clan rule off: the open session follows at once.
        let mut edited = app.settings.worlds[i].clone();
        edited.channel_rules[0].disabled = true;
        app.save_world(Some(i), edited);
        server.write_all(b"[CLAN] Vex: anyone there?\r\n").unwrap();
        frames_until(&mut app, &ctx, |a| text(a, "anyone there?"));
        {
            let log = &app.sessions.get(id).unwrap().tab.channels.log;
            assert_eq!(
                log.tabs().iter().find(|t| t.channel == "clan").unwrap().messages.len(),
                1
            );
        }

        // The demo has no saved world: nothing to teach.
        app.run_command(&ctx, Command::OpenDemo);
        frame(&mut app, &ctx, vec![]);
        let demo = app.active_session.unwrap();
        assert_ne!(demo, id);
        app.actions
            .push(AppAction::MarkChannel(demo, "[CLAN] Vex: meeting at dawn".into(), None));
        frame(&mut app, &ctx, vec![]);
        let mark = app.mark.as_ref().unwrap();
        assert!(!mark.can_teach);
        assert!(!mark.can_save());
        assert_eq!(mark.error, t(S::TeachChannelNoProfile));
        assert_eq!(
            app.teach_channel_rule(demo, ChannelRule::new("clan", "x", None)),
            Err(t(S::TeachChannelNoProfile).to_string())
        );
        app.mark = None;
        app.on_exit();
        drop(app);

        // The rules were saved with the world.
        let again = WandurApp::new(&ctx, offline_options(&dir));
        let rules = &again.settings.worlds[i].channel_rules;
        assert_eq!(rules.len(), 2);
        assert!(rules[0].disabled && rules[1].exclude);
        drop(again);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Save world on a session opened by address (Quick connect): the new saved world is the
    /// session's world at once, so Mark as channel can teach it without reconnecting. Another
    /// session at the same address follows; one elsewhere and the demo do not.
    #[test]
    fn save_world_attaches_the_open_session_at_that_address() {
        use wandur_core::channels::ChannelRule;
        let dir = superpowers_dir("app-save-world-attach");
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                ..offline_options(&dir)
            },
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Endpoint::new("127.0.0.1", port);
        app.push_action(AppAction::Connect(endpoint.clone()));
        frame(&mut app, &ctx, vec![]);
        let id = app.active_session.unwrap();
        app.push_action(AppAction::Connect(endpoint));
        frame(&mut app, &ctx, vec![]);
        let twin = app.active_session.unwrap();
        let elsewhere = Endpoint::new("127.0.0.1", other.local_addr().unwrap().port());
        app.push_action(AppAction::Connect(elsewhere));
        frame(&mut app, &ctx, vec![]);
        let apart = app.active_session.unwrap();
        assert_eq!(app.sessions.get(id).unwrap().tab.world, None);
        assert_eq!(
            app.teach_channel_rule(id, ChannelRule::new("clan", "x", None)),
            Err(t(S::TeachChannelNoProfile).to_string())
        );

        app.push_action(AppAction::SaveWorld(id));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.settings.worlds.len(), 1);
        let world_id = app.settings.worlds[0].world_id.clone();
        assert!(worlds::valid_world_id(&world_id));
        for session in [id, twin] {
            let tab = &app.sessions.get(session).unwrap().tab;
            assert_eq!(tab.world, Some(0), "attached without reconnecting");
            assert_eq!(tab.world_id.as_deref(), Some(world_id.as_str()));
        }
        assert_eq!(app.sessions.get(apart).unwrap().tab.world, None);

        // Mark as channel can teach it now, and the rule is in force for the session.
        app.actions
            .push(AppAction::MarkChannel(id, "[CLAN] Vex: meeting at dawn".into(), None));
        frame(&mut app, &ctx, vec![]);
        assert!(app.mark.as_ref().unwrap().can_teach);
        app.mark = None;
        let rule = ChannelRule::new("clan", r"^\[CLAN\] (?<speaker>\w+): (?<text>.*)$", Some("clan"));
        app.teach_channel_rule(id, rule.clone()).unwrap();
        assert_eq!(app.settings.worlds[0].channel_rules, std::slice::from_ref(&rule));
        assert_eq!(
            app.sessions.get(id).unwrap().tab.channels.rules().rules()[0].channel,
            "clan"
        );

        // Saving again changes nothing: the world is already saved.
        app.push_action(AppAction::SaveWorld(twin));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.settings.worlds.len(), 1);
        app.on_exit();
        drop(app);
        drop((listener, other));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A damaged wandur.db is moved aside, a new one is made, the app starts and says so.
    #[test]
    fn a_corrupt_database_does_not_stop_the_app() {
        let dir = std::env::temp_dir().join(format!("wandur-app-corrupt-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(wandur_core::db::DB_FILE),
            b"this is not a database, honestly".repeat(200),
        )
        .unwrap();
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        frame(&mut app, &ctx, vec![]);
        assert!(app.db.is_some());
        assert!(app.notes.iter().any(|n| n.contains("unreadable")), "{:?}", app.notes);
        assert_eq!(
            app.db.as_ref().unwrap().schema_version().unwrap(),
            wandur_core::db::SCHEMA_VERSION
        );
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scene (an ephemeral run) never opens the data directory's wandur.db: a damaged one is
    /// not moved aside and none is made where there was none. It gets a database of its own,
    /// elsewhere, removed with the app.
    #[test]
    fn an_ephemeral_run_leaves_the_data_directory_database_alone() {
        let damaged = b"this is not a database, honestly".repeat(200);
        for existing in [Some(damaged.clone()), None] {
            let dir = superpowers_dir("app-ephemeral-db");
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join(wandur_core::db::DB_FILE);
            if let Some(bytes) = &existing {
                std::fs::write(&file, bytes).unwrap();
            }
            let ctx = egui::Context::default();
            let mut app = WandurApp::new(
                &ctx,
                Options {
                    ephemeral: true,
                    ..offline_options(&dir)
                },
            );
            frame(&mut app, &ctx, vec![]);
            let scratch = app.db.as_ref().expect("a database of its own").path().to_path_buf();
            assert!(!scratch.starts_with(&dir), "{}", scratch.display());
            assert!(!app.notes.iter().any(|n| n.contains("unreadable")), "{:?}", app.notes);
            app.shutdown();
            drop(app);
            assert!(!scratch.exists(), "the scene's database is removed with it");
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(wandur_core::db::DB_FILE))
                .collect();
            names.sort();
            match &existing {
                Some(bytes) => {
                    assert_eq!(names, [wandur_core::db::DB_FILE]);
                    assert_eq!(&std::fs::read(&file).unwrap(), bytes, "left exactly as it was");
                }
                None => assert!(names.is_empty(), "{names:?}"),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Unpin Map through the app: it moves to the right strip; a click on its strip tab slides
    /// it out; Escape hides it; Pin puts it back; the unpinned state survives a restart.
    #[test]
    fn pinning_and_auto_hide_through_the_app() {
        let dir = std::env::temp_dir().join(format!("wandur-autohide-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let options = || Options {
            data_dir: Some(dir.clone()),
            directory_url: Some("http://127.0.0.1:9".into()),
            fetcher: Some(Arc::new(Offline)),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, options());
        // The C# layout: Map in the right column.
        app.apply_preset(workspace::Preset::Both);
        frame(&mut app, &ctx, vec![]);
        assert!(app.dock_rect.is_positive());
        app.actions.push(AppAction::Unpin(Tab::Map));
        frame(&mut app, &ctx, vec![]);
        assert!(app.auto.is_hidden(Tab::Map));
        assert_eq!(app.auto.hidden[0].edge, Edge::Right);
        assert!(app.dock.find_tab(&Tab::Map).is_none());
        frame(&mut app, &ctx, vec![]);
        let strip = app
            .strip_tabs
            .iter()
            .find(|(t, _)| *t == Tab::Map)
            .expect("a strip tab")
            .1;
        assert!(strip.left() > 1100.0, "on the right edge: {strip:?}");
        for events in click(strip.center()) {
            frame(&mut app, &ctx, events);
        }
        assert_eq!(app.auto.shown.map(|s| (s.tab, s.by_click)), Some((Tab::Map, true)));
        // Moving away does not hide a clicked-open panel; Escape does.
        frame(&mut app, &ctx, vec![Event::PointerMoved(pos2(500.0, 400.0))]);
        assert!(app.auto.shown.is_some());
        let escape = Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(&mut app, &ctx, vec![escape]);
        assert!(app.auto.shown.is_none());
        // Restart: still unpinned.
        app.on_exit();
        drop(app);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, options());
        assert!(
            app.auto.is_hidden(Tab::Map),
            "the unpinned state was saved with the layout"
        );
        frame(&mut app, &ctx, vec![]);
        app.actions.push(AppAction::Pin(Tab::Map));
        frame(&mut app, &ctx, vec![]);
        assert!(!app.auto.is_hidden(Tab::Map));
        let map = app.dock.find_tab(&Tab::Map).expect("docked again");
        let channels = app.dock.find_tab(&Tab::Channels).unwrap();
        assert_ne!(
            map.node_path(),
            channels.node_path(),
            "back above Channels, not in its leaf"
        );
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every piece of text drawn in a frame.
    fn drawn_text(app: &mut WandurApp, ctx: &egui::Context) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        // A new popup or dialog is sized in an invisible first frame; draw twice.
        frame(app, ctx, vec![]);
        let mut eframe_frame = eframe::Frame::_new_kittest();
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx(), &mut eframe_frame);
            app.ui(ui, &mut eframe_frame);
        });
        out.textures_delta.clear();
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Choosing German in Settings redraws the menus, toolbar and dialog in German at once,
    /// without touching the open connection; Cancel brings English back; Save keeps a choice.
    #[test]
    fn the_language_switches_live_and_cancel_restores_it() {
        use std::net::TcpListener;
        let dir = std::env::temp_dir().join(format!("wandur-app-language-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                connect: vec![endpoint],
                language: Some(Language::En),
                ..offline_options(&dir)
            },
        );
        let (_server, _) = listener.accept().unwrap();
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.sessions.connected() == 0 {
            assert!(Instant::now() < deadline, "the session connects");
            frame(&mut app, &ctx, vec![]);
            std::thread::sleep(Duration::from_millis(5));
        }
        let session = app.active_session.unwrap();
        // The menus show in the title bar's menu button (on macOS also in the menu bar at the top
        // of the screen, which a test cannot draw).
        app.menu_button.toggle();
        app.settings.language = "en".into();
        app.actions.push(AppAction::OpenSettings);
        frame(&mut app, &ctx, vec![]);
        let english = drawn_text(&mut app, &ctx);
        for label in ["File", "Interface language", "Save preferences", "Find a MUD"] {
            assert!(english.iter().any(|t| t == label), "{label} in {english:?}");
        }
        app.settings_dialog.as_mut().unwrap().choose_language("de");
        let german = drawn_text(&mut app, &ctx);
        for label in [
            text_in(Language::De, S::File),
            text_in(Language::De, S::InterfaceLanguage),
            text_in(Language::De, S::SavePreferences),
            text_in(Language::De, S::FindAMUD),
            "Deutsch",
        ] {
            assert!(german.iter().any(|t| t == label), "{label} in {german:?}");
        }
        assert!(!german.iter().any(|t| t == "Interface language"));
        // The session is the same, still connected, and nothing reconnected.
        assert_eq!(app.active_session, Some(session));
        assert!(app.sessions.get(session).unwrap().tab.is_connected());
        assert!(listener.accept().is_err(), "no second connection");
        let result = app.settings_dialog.as_ref().unwrap().cancel();
        assert_eq!(result, SettingsResult::Cancelled);
        app.settings_dialog = None;
        let back = drawn_text(&mut app, &ctx);
        assert!(back.iter().any(|t| t == "File"), "{back:?}");
        assert_eq!(app.settings.language, "en");
        // Save keeps a choice.
        app.settings_dialog = Some(SettingsDialog::new(&app.settings));
        app.settings_dialog.as_mut().unwrap().choose_language("fr");
        let draft = app.settings_dialog.take().unwrap().draft;
        app.save_preferences(&ctx, draft);
        assert_eq!(app.settings.language, "fr");
        assert!(
            drawn_text(&mut app, &ctx)
                .iter()
                .any(|t| t == text_in(Language::Fr, S::File))
        );
        assert!(app.sessions.get(session).unwrap().tab.is_connected());
        l10n::override_thread(None);
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// File > Open Offline Demo, a walk, File > Save Transcript to the chosen file, then
    /// Session > Clear Transcript; the menu items follow whether there is text.
    #[test]
    fn demo_save_and_clear_through_the_menus() {
        let dir = std::env::temp_dir().join(format!("wandur-app-demo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("chosen.txt");
        let chosen = file.clone();
        let suggested = Arc::new(std::sync::Mutex::new(String::new()));
        let seen = Arc::clone(&suggested);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                choose_file: Some(Arc::new(move |name: &str| {
                    *seen.lock().unwrap() = name.to_string();
                    Some(chosen.clone())
                })),
                ..offline_options(&dir)
            },
        );
        assert!(!app.menu_state().has_text);
        app.run_command(&ctx, Command::OpenDemo);
        frame(&mut app, &ctx, vec![]);
        let id = app.active_session.expect("the demo is the active session");
        assert!(app.sessions.get(id).unwrap().tab.is_demo());
        assert!(app.menu_state().has_text && app.menu_state().can_disconnect);
        for command in ["north", "up"] {
            let tab = &mut app.sessions.get_mut(id).unwrap().tab;
            tab.input = command.into();
            tab.submit();
        }
        assert_eq!(app.sessions.get(id).unwrap().tab.title(), "Above the Rooftops");
        app.run_command(&ctx, Command::SaveTranscript);
        let saved = std::fs::read_to_string(&file).unwrap();
        let transcript = app.sessions.get(id).unwrap().tab.terminal.transcript();
        assert_eq!(saved, format!("{transcript}\n"));
        assert!(saved.contains("A swallow has made a home inside the silent bell."));
        let name = suggested.lock().unwrap().clone();
        assert!(name.starts_with("wandur-") && name.ends_with(".txt"), "{name}");
        app.run_command(&ctx, Command::ClearTranscript);
        assert!(!app.sessions.get(id).unwrap().tab.terminal.has_text());
        assert!(!app.menu_state().has_text, "Save and Clear are disabled again");
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Help > Getting Started and Other MUD Clients ask first, then open the site's pages
    /// through the launcher (a fake here); a browser that does not open is a notice.
    #[test]
    fn help_links_ask_before_opening() {
        let dir = std::env::temp_dir().join(format!("wandur-app-help-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let launched = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let record = Arc::clone(&launched);
        let opens = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let answer = Arc::clone(&opens);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                site: Some("http://127.0.0.1:9/".into()),
                launcher: Some(Arc::new(move |url: &str| {
                    record.lock().unwrap().push(url.to_string());
                    answer.load(std::sync::atomic::Ordering::Relaxed)
                })),
                ..offline_options(&dir)
            },
        );
        app.run_command(&ctx, Command::GettingStarted);
        assert_eq!(app.dialog, Some(Dialog::Link("http://127.0.0.1:9/client/help".into())));
        let texts = drawn_text(&mut app, &ctx);
        assert!(texts.iter().any(|t| t == "Open this link?"), "{texts:?}");
        assert!(launched.lock().unwrap().is_empty(), "nothing opens before the answer");
        app.dialog = None;
        app.open_link("http://127.0.0.1:9/client/help");
        app.run_command(&ctx, Command::OtherClients);
        assert_eq!(app.dialog, Some(Dialog::Link("http://127.0.0.1:9/clients".into())));
        app.dialog = None;
        app.open_link("http://127.0.0.1:9/clients");
        assert_eq!(
            *launched.lock().unwrap(),
            ["http://127.0.0.1:9/client/help", "http://127.0.0.1:9/clients"]
        );
        assert!(app.notes.is_empty());
        opens.store(false, std::sync::atomic::Ordering::Relaxed);
        app.open_link("http://127.0.0.1:9/clients");
        assert_eq!(app.notes.first().map(String::as_str), Some(t(S::LinkNotOpened)));
        app.run_command(&ctx, Command::About);
        assert_eq!(app.dialog, Some(Dialog::About));
        let texts = drawn_text(&mut app, &ctx);
        assert!(texts.iter().any(|t| t == crate::dialogs::DISPLAY_NAME), "{texts:?}");
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// View > Layout applies a preset at once, keeps the open sessions (the active one stays
    /// active), and saves it as the layout the next start has; Restore Panels goes back to the
    /// new-install layout.
    #[test]
    fn layout_presets_apply_keep_sessions_and_persist() {
        let dir = std::env::temp_dir().join(format!("wandur-app-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        frame(&mut app, &ctx, vec![]);
        app.push_action(AppAction::Connect(Endpoint::new("127.0.0.1", port)));
        frame(&mut app, &ctx, vec![]);
        let session = app.active_session.unwrap();
        let focus = workspace::Preset::ALL
            .iter()
            .position(|p| *p == workspace::Preset::Focus)
            .unwrap();
        app.run_command(&ctx, Command::Layout(focus as u8));
        frame(&mut app, &ctx, vec![]);
        assert!(app.dock.find_tab(&Tab::Session(session)).is_some());
        assert_eq!(app.active_session, Some(session));
        assert!(app.dock.find_tab(&Tab::Map).is_none() && app.auto.is_hidden(Tab::Map));
        app.on_exit();
        drop(app);
        let saved = std::fs::read_to_string(dir.join(layout::LAYOUT_FILE)).unwrap();
        let (focus_dock, focus_auto) = workspace::preset_layout(workspace::Preset::Focus);
        assert_eq!(Some(saved.clone()), layout::to_json(&focus_dock, &focus_auto));
        // The next start reads it back.
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        frame(&mut app, &ctx, vec![]);
        assert!(app.auto.is_hidden(Tab::Channels) && app.dock.find_tab(&Tab::Channels).is_none());
        // Restore Panels: the new-install layout (Panels on the left).
        app.run_command(&ctx, Command::RestorePanels);
        frame(&mut app, &ctx, vec![]);
        let (left, _) = workspace::preset_layout(workspace::NEW_INSTALL);
        assert_eq!(
            layout::to_json(&app.dock, &app.auto),
            layout::to_json(&left, &AutoHide::default())
        );
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The toolbar's world picker stays open for a click in its address field (it used to close),
    /// and Enter there connects to the typed address and closes it.
    #[test]
    fn the_world_picker_stays_open_for_its_address_field_and_enter_connects() {
        let dir = std::env::temp_dir().join(format!("wandur-app-picker-field-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        app.settings.worlds = vec![SavedWorld {
            name: "The Lantern Road".into(),
            ..SavedWorld::from_endpoint(String::from("127.0.0.1"), &Endpoint::new("127.0.0.1", 4400))
        }];
        let click = |at: egui::Pos2| {
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            vec![Event::PointerMoved(at), button(true), button(false)]
        };
        frame(&mut app, &ctx, vec![]);
        let picker = app.picker_rects.0;
        assert!(picker.is_positive(), "the toolbar shows the picker");
        frame(&mut app, &ctx, click(picker.center()));
        frame(&mut app, &ctx, vec![]);
        let field = app.picker_rects.1;
        assert!(field.is_positive(), "the popup is open with its address field");
        // A click in the field used to close the popup; now it stays open and the field has focus.
        frame(&mut app, &ctx, click(field.center()));
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert!(app.picker_rects.1.is_positive(), "the popup stays open for its field");
        frame(&mut app, &ctx, vec![Event::Text("mud.example.test:4000".into())]);
        assert_eq!(app.address, "mud.example.test:4000");
        frame(
            &mut app,
            &ctx,
            vec![Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.sessions.len(), 1, "Enter connects to the typed address");
        assert_eq!(app.connect_error, None);
        frame(&mut app, &ctx, vec![]);
        assert!(!app.picker_rects.1.is_positive(), "connecting closes the popup");
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The toolbar's world picker shows the active session's world, and follows when another
    /// session becomes active; a world chosen in it stays until then, and Connect opens it in a
    /// new session.
    #[test]
    fn the_world_picker_follows_the_active_session() {
        let dir = std::env::temp_dir().join(format!("wandur-app-picker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        let world = |name: &str| SavedWorld {
            name: name.into(),
            ..SavedWorld::from_endpoint(String::from("127.0.0.1"), &Endpoint::new("127.0.0.1", port))
        };
        app.settings.worlds = vec![world("The Lantern Road"), world("Starfall Reach"), world("Emberwake")];
        app.panel.selected_world = Some(2);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.panel.selected_world, Some(2), "no session: the choice stays");
        app.push_action(AppAction::ConnectWorld(0));
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let lantern = app.active_session.unwrap();
        assert_eq!(app.panel.selected_world, Some(0), "the active session's world");
        assert!(app.picker_on_active_world());
        // Choosing another world keeps it chosen; Connect opens it as a new session.
        app.panel.selected_world = Some(1);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.panel.selected_world, Some(1));
        assert!(!app.picker_on_active_world());
        assert_eq!(app.connect_hint(), tf(S::PickerConnectTo, &[&"Starfall Reach"]));
        app.run_command(&ctx, Command::ConnectSelected);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.sessions.len(), 2);
        let starfall = app.active_session.unwrap();
        assert_ne!(starfall, lantern);
        assert_eq!(app.panel.selected_world, Some(1));
        // Back to the first session's tab: the picker follows.
        workspace::focus_tab(&mut app.dock, Tab::Session(lantern));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.active_session, Some(lantern));
        assert_eq!(app.panel.selected_world, Some(0));
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// View toggles close and reopen the side panels; the menu's check marks follow.
    #[test]
    fn saved_worlds_connects_or_goes_back_duplicates_and_asks_before_deleting() {
        let dir = std::env::temp_dir().join(format!("wandur-app-saved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A loopback listener keeps the sessions open (connected, never closed by the server).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        let lantern = SavedWorld {
            name: "The Lantern Road".into(),
            password_id: Some("secret-ref".into()),
            auto_login: true,
            ..SavedWorld::from_endpoint(String::from("127.0.0.1"), &Endpoint::new("127.0.0.1", port))
        };
        let other = SavedWorld {
            name: "Starfall Reach".into(),
            ..SavedWorld::from_endpoint(String::from("127.0.0.1"), &Endpoint::new("127.0.0.1", port))
        };
        app.settings.worlds = vec![lantern, other];
        assert!(
            app.panel_visible(Tab::SavedWorlds),
            "a panel of its own in the default layout"
        );
        frame(&mut app, &ctx, vec![]);

        // Connect (double-click, Enter, the menu's Connect) opens a session...
        app.push_action(AppAction::GoToWorld(0));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.sessions.len(), 1);
        let first = app.active_session.unwrap();
        // ...and goes back to it the second time instead of logging in twice.
        app.push_action(AppAction::OpenDirectory);
        frame(&mut app, &ctx, vec![]);
        app.push_action(AppAction::GoToWorld(0));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.active_session, Some(first));
        assert!(!app.directory_active);
        // Connect in new tab always opens another session.
        app.push_action(AppAction::ConnectWorld(0));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.sessions.len(), 2);
        // A session of the second world keeps pointing at it after a duplicate is inserted.
        app.push_action(AppAction::ConnectWorld(1));
        frame(&mut app, &ctx, vec![]);
        let starfall = app.active_session.unwrap();

        // Duplicate: a copy after the original, selected, without the saved password.
        app.push_action(AppAction::DuplicateWorld(0));
        frame(&mut app, &ctx, vec![]);
        let names: Vec<&str> = app.settings.worlds.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["The Lantern Road", "The Lantern Road (copy)", "Starfall Reach"]);
        let copy = &app.settings.worlds[1];
        assert!(copy.password_id.is_none() && !copy.auto_login);
        assert_eq!((copy.host.as_str(), copy.port), ("127.0.0.1", port));
        assert_eq!(app.panel.selected_world, Some(1));
        assert_eq!(app.sessions.get(starfall).unwrap().tab.world, Some(2));

        // Explore in directory does nothing for a world the directory does not list.
        app.push_action(AppAction::ExploreWorld(0));
        frame(&mut app, &ctx, vec![]);
        assert!(app.dir_view.exploring.is_none());

        // Delete on the focused list: a world no session came from goes at once, with the undo
        // toast; Undo puts it back exactly as it was, at its place.
        let copy = app.settings.worlds[1].clone();
        app.panel.focus_row = Some(1);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let delete = Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, &ctx, vec![delete.clone()]);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(
            app.panel.pending_delete, None,
            "no question for a world without a session"
        );
        assert_eq!(app.settings.worlds.len(), 2);
        let toast = app.toast.as_ref().expect("the undo toast");
        assert_eq!(toast.text, "Deleted The Lantern Road (copy)");
        app.push_action(AppAction::UndoToast(toast.serial));
        frame(&mut app, &ctx, vec![]);
        assert!(app.toast.is_none());
        assert_eq!(app.settings.worlds.len(), 3);
        assert_eq!(app.settings.worlds[1], copy, "restored exactly, at its place");
        assert_eq!(app.sessions.get(starfall).unwrap().tab.world, Some(2));
        // A world with an open session asks first; nothing is removed until confirmed. Undo
        // gives the session its world back.
        app.panel.selected_world = Some(2);
        app.panel.focus_row = Some(2);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![delete]);
        assert_eq!(app.panel.pending_delete, Some(2));
        assert_eq!(app.settings.worlds.len(), 3);
        app.push_action(AppAction::DeleteWorld(2));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.settings.worlds.len(), 2);
        assert_eq!(app.sessions.get(starfall).unwrap().tab.world, None);
        let serial = app.toast.as_ref().unwrap().serial;
        app.push_action(AppAction::UndoToast(serial));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.settings.worlds[2].name, "Starfall Reach");
        assert_eq!(app.sessions.get(starfall).unwrap().tab.world, Some(2));
        // A new delete replaces the toast: the old one's Undo does nothing any more.
        app.panel.pending_delete = None;
        app.push_action(AppAction::DeleteWorld(1));
        frame(&mut app, &ctx, vec![]);
        let first_toast = app.toast.as_ref().unwrap().serial;
        app.push_action(AppAction::DeleteWorld(0));
        frame(&mut app, &ctx, vec![]);
        assert_ne!(app.toast.as_ref().unwrap().serial, first_toast);
        assert_eq!(app.toast.as_ref().unwrap().text, "Deleted The Lantern Road");
        app.push_action(AppAction::UndoToast(first_toast));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.settings.worlds.len(), 1, "the replaced toast cannot undo");
        let serial = app.toast.as_ref().unwrap().serial;
        app.push_action(AppAction::UndoToast(serial));
        frame(&mut app, &ctx, vec![]);
        let names: Vec<&str> = app.settings.worlds.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["The Lantern Road", "Starfall Reach"]);
        assert_eq!(app.settings.worlds[0].password_id.as_deref(), Some("secret-ref"));
        assert_eq!(app.sessions.get(starfall).unwrap().tab.world, Some(1));
        // Enter on the focused list connects the selection (here: back to Starfall's session).
        app.panel.pending_delete = None;
        app.panel.selected_world = Some(1);
        app.panel.focus_row = Some(1);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let enter = Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, &ctx, vec![enter]);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.sessions.len(), 3);
        assert_eq!(app.active_session, Some(starfall));
        drop(listener);
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dragging_a_header_onto_a_guide_docks_the_panel_at_its_previewed_size() {
        let dir = std::env::temp_dir().join(format!("wandur-app-drag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        app.run_command(&ctx, Command::OpenDemo);
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        let header = app.scene_target(crate::scene::Target::Header(Tab::Channels)).unwrap();
        let leaf = app.scene_target(crate::scene::Target::Session).unwrap();
        let tile = crate::dock_drop::compass(leaf)
            .into_iter()
            .find(|(s, _)| *s == Some(egui_dock::Split::Left))
            .unwrap()
            .1;
        let previewed = leaf.width() / 2.0;
        let start = pos2(header.left() + 60.0, header.center().y);
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, &ctx, vec![Event::PointerMoved(start)]);
        frame(&mut app, &ctx, vec![button(start, true)]);
        for n in 1..=20 {
            let at = start + (tile.center() - start) * (n as f32 / 20.0);
            frame(&mut app, &ctx, vec![Event::PointerMoved(at)]);
        }
        // While it hovers the guide, the preview is half the session's panel.
        match app.drop.dragging {
            Some((Tab::Channels, Some(target))) => {
                assert_eq!(crate::dock_drop::preview(target, app.dock_rect).width(), previewed)
            }
            other => panic!("{other:?}"),
        }
        frame(&mut app, &ctx, vec![button(tile.center(), false)]);
        for _ in 0..4 {
            frame(&mut app, &ctx, vec![]);
        }
        let channels = app.dock.find_tab(&Tab::Channels).unwrap();
        let session = app.dock.find_tab_from(|t| matches!(t, Tab::Session(_))).unwrap();
        assert_eq!(
            channels.node.parent(),
            session.node.parent(),
            "split beside the session"
        );
        assert_eq!(channels.node, channels.node.parent().unwrap().left(), "on its left");
        let width = app.dock.leaf(channels.node_path()).unwrap().rect.width();
        assert!(
            (width - previewed).abs() <= 3.0,
            "docked {width}, previewed {previewed}"
        );
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_system_caption_shows_the_window_title_and_the_drawn_skins_their_plate() {
        let dir = std::env::temp_dir().join(format!("wandur-app-caption-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                skin: Some("System".into()),
                ..offline_options(&dir)
            },
        );
        app.run_command(&ctx, Command::OpenDemo);
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let title = app.title.clone();
        assert!(title.ends_with(" · Wandur Mud Client"), "{title}");
        // The title bar has its menu button (no menu bar in the window on any platform).
        assert!(app.menu_button.button.is_some(), "the title bar's menu button");
        let texts = drawn_text(&mut app, &ctx);
        assert!(texts.contains(&title), "the caption shows {title}: {texts:?}");
        assert!(
            !texts.iter().any(|t| t.starts_with("WANDUR MUD CLIENT")),
            "no plate in System"
        );
        app.set_skin(&ctx, SkinId::Fleet);
        frame(&mut app, &ctx, vec![]);
        let texts = drawn_text(&mut app, &ctx);
        assert!(texts.iter().any(|t| t.starts_with("WANDUR MUD CLIENT")), "{texts:?}");
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One frame in which the window reports `full_screen`.
    fn frame_full_screen(app: &mut WandurApp, ctx: &egui::Context, full_screen: bool) {
        let mut eframe_frame = eframe::Frame::_new_kittest();
        let mut input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().fullscreen = Some(full_screen);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx(), &mut eframe_frame);
            app.ui(ui, &mut eframe_frame);
        });
        out.textures_delta.clear();
    }

    /// Full screen left with the green button or Esc (not the app's command) brings the band
    /// back, and the traffic lights with it; the app's own toggle is not undone mid-transition.
    #[test]
    fn full_screen_follows_the_window_however_it_was_left() {
        let dir = std::env::temp_dir().join(format!("wandur-app-fullscreen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                skin: Some("Armored".into()),
                ..offline_options(&dir)
            },
        );
        frame_full_screen(&mut app, &ctx, false);
        assert!(!app.full_screen);
        app.run_command(&ctx, Command::FullScreen);
        assert!(app.full_screen);
        // The window has not got there yet: the toggle stands.
        frame_full_screen(&mut app, &ctx, false);
        assert!(app.full_screen);
        frame_full_screen(&mut app, &ctx, true);
        assert!(app.full_screen);
        // Left with the green button.
        frame_full_screen(&mut app, &ctx, false);
        assert!(!app.full_screen);
        // And entered with it.
        frame_full_screen(&mut app, &ctx, true);
        assert!(app.full_screen);
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn key_event(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    /// One frame with these modifiers held (egui reads Alt from the input's modifiers).
    fn frame_with(app: &mut WandurApp, ctx: &egui::Context, events: Vec<Event>, modifiers: egui::Modifiers) {
        let mut eframe_frame = eframe::Frame::_new_kittest();
        let mut all = vec![Event::ModifiersChanged(modifiers)];
        all.extend(events);
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            events: all,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx(), &mut eframe_frame);
            app.ui(ui, &mut eframe_frame);
        });
        out.textures_delta.clear();
    }

    #[test]
    fn windows_and_linux_have_a_menu_button_that_alt_f10_and_alt_f_open() {
        let dir = std::env::temp_dir().join(format!("wandur-app-menubutton-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                platform: Some(Platform::Other),
                ..offline_options(&dir)
            },
        );
        let none = egui::Modifiers::NONE;
        frame(&mut app, &ctx, vec![]);
        // No menu shows until asked: no menu bar in the window.
        assert!(!app.menu_button.open);
        let titles = [S::File, S::Edit, S::View, S::Session, S::Window, S::Help];
        let shown = |app: &mut WandurApp, ctx: &egui::Context| {
            let texts = drawn_text(app, ctx);
            titles
                .iter()
                .filter(|s| texts.iter().any(|t| t == text_in(Language::En, **s)))
                .count()
        };
        assert_eq!(shown(&mut app, &ctx), 0);
        // The title bar's button (top left on Windows and Linux) opens the whole model.
        let button = app.menu_button.button.expect("drawn");
        assert!(button.left() < 100.0, "top left: {button:?}");
        for events in click(button.center()) {
            frame(&mut app, &ctx, events);
        }
        assert!(app.menu_button.open && !app.menu_button.keyboard);
        assert_eq!(shown(&mut app, &ctx), 6, "File, Edit, View, Session, Window, Help");
        frame(&mut app, &ctx, vec![key_event(egui::Key::Escape, true, none)]);
        assert!(!app.menu_button.open, "Escape closes it");
        // Alt alone (down, then up with nothing else) opens it with the first item lit.
        frame_with(&mut app, &ctx, vec![], egui::Modifiers::ALT);
        frame_with(&mut app, &ctx, vec![], none);
        assert!(app.menu_button.open && app.menu_button.keyboard);
        assert_eq!(app.menu_button.cursor, [Some(0)]);
        // Arrows and Enter walk it: Down, Down, Right opens View on its first item (Workspace).
        for key in [egui::Key::ArrowDown, egui::Key::ArrowDown, egui::Key::ArrowRight] {
            frame(&mut app, &ctx, vec![key_event(key, true, none)]);
        }
        assert_eq!(app.menu_button.path, [2]);
        assert!(!app.panel_visible(Tab::Workspace), "closed in a new install's layout");
        frame(&mut app, &ctx, vec![key_event(egui::Key::Enter, true, none)]);
        assert!(!app.menu_button.open);
        assert!(app.panel_visible(Tab::Workspace), "View > Workspace ran");
        // Alt used with another key is not a tap.
        frame_with(
            &mut app,
            &ctx,
            vec![key_event(egui::Key::Q, true, egui::Modifiers::ALT)],
            egui::Modifiers::ALT,
        );
        frame_with(&mut app, &ctx, vec![], none);
        assert!(!app.menu_button.open);
        // F10 and Alt+F open it too.
        frame(&mut app, &ctx, vec![key_event(egui::Key::F10, true, none)]);
        assert!(app.menu_button.open && app.menu_button.cursor == [Some(0)]);
        frame(&mut app, &ctx, vec![key_event(egui::Key::Escape, true, none)]);
        assert!(!app.menu_button.open);
        frame_with(
            &mut app,
            &ctx,
            vec![key_event(egui::Key::F, true, egui::Modifiers::ALT)],
            egui::Modifiers::ALT,
        );
        assert!(app.menu_button.open);
        frame_with(&mut app, &ctx, vec![key_event(egui::Key::Escape, true, none)], none);
        assert!(!app.menu_button.open);
        // Shortcuts work with no menu showing: Ctrl+T is Find a MUD.
        app.push_action(AppAction::Notice(String::new()));
        let ctrl = egui::Modifiers::CTRL;
        frame_with(&mut app, &ctx, vec![key_event(egui::Key::T, true, ctrl)], ctrl);
        frame(&mut app, &ctx, vec![]);
        assert!(app.directory_active, "Ctrl+T opened Find a MUD");
        assert!(!app.menu_button.open);
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn macos_has_the_menu_button_too_beside_its_menu_bar() {
        let dir = std::env::temp_dir().join(format!("wandur-app-menubutton-mac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                platform: Some(Platform::Mac),
                skin: Some("System".into()),
                ..offline_options(&dir)
            },
        );
        frame(&mut app, &ctx, vec![]);
        let button = app.menu_button.button.expect("drawn");
        assert!(
            button.right() > 1100.0,
            "the last title control, clear of the traffic lights: {button:?}"
        );
        for events in click(button.center()) {
            frame(&mut app, &ctx, events);
        }
        assert!(app.menu_button.open);
        let texts = drawn_text(&mut app, &ctx);
        for s in [S::File, S::View, S::Help, S::AboutWandur, S::QuitWandur] {
            assert!(
                texts.iter().any(|t| t == text_in(Language::En, s)),
                "{s:?} in {texts:?}"
            );
        }
        // Alt and F10 belong to the macOS menu bar, not this button.
        frame(
            &mut app,
            &ctx,
            vec![key_event(egui::Key::Escape, true, egui::Modifiers::NONE)],
        );
        frame(
            &mut app,
            &ctx,
            vec![key_event(egui::Key::F10, true, egui::Modifiers::NONE)],
        );
        assert!(!app.menu_button.open);
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn view_menu_toggles_panels() {
        let dir = std::env::temp_dir().join(format!("wandur-app-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(&ctx, offline_options(&dir));
        frame(&mut app, &ctx, vec![]);
        assert!(app.menu_state().map_visible && app.menu_state().channels_visible);
        app.run_command(&ctx, Command::ToggleMap);
        assert!(!app.menu_state().map_visible);
        app.run_command(&ctx, Command::ToggleMap);
        assert!(app.menu_state().map_visible);
        app.run_command(&ctx, Command::ToggleToolbar);
        assert!(!app.menu_state().toolbar_visible);
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The live view's divider dragged in the app: the share it settles on is saved in the
    /// settings file (TranscriptTailTests, the saved share); a link in the transcript opens
    /// through the bar's Open, and a refused one becomes a notice.
    #[test]
    fn the_live_view_share_is_saved_and_transcript_links_open_after_asking() {
        let dir = superpowers_dir("app-tail-share");
        let launched = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let record = Arc::clone(&launched);
        let ctx = egui::Context::default();
        let mut app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                launcher: Some(Arc::new(move |url: &str| {
                    record.lock().unwrap().push(url.to_string());
                    true
                })),
                ..offline_options(&dir)
            },
        );
        assert_eq!(app.settings.scroll_tail_share, 0.25);
        app.run_command(&ctx, Command::OpenDemo);
        frame(&mut app, &ctx, vec![]);
        let id = app.active_session.unwrap();
        {
            let entry = app.sessions.get_mut(id).unwrap();
            for i in 0..200 {
                entry.tab.terminal.feed(format!("Line {i}\n").as_bytes());
            }
            entry.tab.terminal.scroll(5);
        }
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let divider = app
            .sessions
            .get(id)
            .unwrap()
            .view
            .divider_rect
            .expect("the live view is open");
        let from = divider.center();
        let to = from - vec2(0.0, 90.0);
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, &ctx, vec![Event::PointerMoved(from), button(from, true)]);
        frame(&mut app, &ctx, vec![Event::PointerMoved(from - vec2(0.0, 10.0))]);
        frame(&mut app, &ctx, vec![Event::PointerMoved(to)]);
        frame(&mut app, &ctx, vec![button(to, false)]);
        frame(&mut app, &ctx, vec![]);
        let share = app.settings.scroll_tail_share;
        assert!(share > 0.3 && share <= 0.6, "{share}");

        // A link: the bar asks, Open opens it through the launcher.
        let view = &mut app.sessions.get_mut(id).unwrap().view;
        assert_eq!(
            crate::terminal_view::request_link(view, "https://lanternroad.example.org/map"),
            Ok(())
        );
        frame(&mut app, &ctx, vec![]);
        frame(&mut app, &ctx, vec![]);
        let (open, _) = app.sessions.get(id).unwrap().view.link_buttons.expect("the bar");
        for events in click(open.center()) {
            frame(&mut app, &ctx, events);
        }
        frame(&mut app, &ctx, vec![]);
        assert_eq!(*launched.lock().unwrap(), ["https://lanternroad.example.org/map"]);

        app.on_exit();
        drop(app);
        let (saved, _) = Settings::load(&dir, wandur_term::MAX_SCROLLBACK);
        assert_eq!(saved.scroll_tail_share, share, "the share was saved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Session history through the app (the C# HistoryNoticeTests and WorkspaceController.History).

    fn history_app(dir: &std::path::Path, clock: Option<Clock>) -> (egui::Context, WandurApp) {
        let ctx = egui::Context::default();
        let app = WandurApp::new(
            &ctx,
            Options {
                language: Some(Language::En),
                history_clock: clock,
                ..offline_options(dir)
            },
        );
        (ctx, app)
    }

    fn join_history(app: &mut WandurApp) {
        for entry in app.sessions.iter_mut() {
            app.history_drains.extend(entry.tab.take_history_drains());
        }
        for handle in std::mem::take(&mut app.history_drains) {
            handle.join().unwrap();
        }
    }

    fn strips(app: &WandurApp) -> Vec<Option<StripNotice>> {
        app.sessions.iter().map(|e| e.tab.strip.clone()).collect()
    }

    /// Don't show again is saved (a restart keeps it), hides the reminder in every session, and
    /// neither stops recording nor hides other notices.
    #[test]
    fn dont_show_again_survives_restart_and_other_tabs_without_disabling_recording() {
        let dir = superpowers_dir("app-history-notice");
        let (ctx, mut app) = history_app(&dir, None);
        app.open_demo();
        app.open_demo();
        frame(&mut app, &ctx, vec![]);
        assert_eq!(
            strips(&app),
            [Some(StripNotice::HistoryRecording), Some(StripNotice::HistoryRecording)]
        );
        let texts = drawn_text(&mut app, &ctx);
        assert!(texts.iter().any(|x| x == t(S::HistoryRecordingNotice)), "{texts:?}");
        assert!(texts.iter().any(|x| x == t(S::DontShowAgain)));
        app.actions.push(AppAction::HideHistoryNotice);
        frame(&mut app, &ctx, vec![]);
        assert_eq!(strips(&app), [None, None]);
        assert!(app.settings.hide_history_recording_notice);
        assert!(app.settings.history_enabled);
        let id = app.active_session.unwrap();
        let tab = &mut app.sessions.get_mut(id).unwrap().tab;
        assert!(tab.is_recording());
        tab.input = "persistedhistoryprobe".into();
        tab.submit();
        tab.flush_history()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let store = app.history_store.clone().unwrap();
        let hits = store
            .search("persistedhistoryprobe", &Default::default(), 0, 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.kind, wandur_core::history::EntryKind::Sent);
        assert_eq!(hits[0].session.world_key, "demo");
        app.on_exit();
        drop(app);

        // Reopened from the saved settings.
        let (ctx, mut app) = history_app(&dir, None);
        assert!(app.settings.hide_history_recording_notice);
        app.open_demo();
        frame(&mut app, &ctx, vec![]);
        assert_eq!(strips(&app), [None]);
        let id = app.active_session.unwrap();
        assert!(app.sessions.get(id).unwrap().tab.is_recording());
        // A recording failure still shows, without the checkbox.
        app.sessions.get_mut(id).unwrap().tab.strip = Some(StripNotice::Text(t(S::HistoryRecordingFailed).into()));
        let texts = drawn_text(&mut app, &ctx);
        assert!(texts.iter().any(|x| x == t(S::HistoryRecordingFailed)));
        assert!(!texts.iter().any(|x| x == t(S::DontShowAgain)));
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed save does not pretend the reminder was turned off.
    #[test]
    fn a_failed_preference_save_keeps_the_reminder() {
        let dir = superpowers_dir("app-history-notice-fail");
        let (ctx, mut app) = history_app(&dir, None);
        app.open_demo();
        frame(&mut app, &ctx, vec![]);
        // The settings file cannot be written: its name is taken by a folder.
        let _ = std::fs::remove_file(dir.join("settings.json"));
        std::fs::create_dir_all(dir.join("settings.json")).unwrap();
        app.actions.push(AppAction::HideHistoryNotice);
        frame(&mut app, &ctx, vec![]);
        assert!(!app.settings.hide_history_recording_notice);
        assert_eq!(
            strips(&app),
            [Some(StripNotice::Text(t(S::HistoryNoticePreferenceFailed).into()))]
        );
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The close button dismisses this reminder only; the next connection shows it again.
    #[test]
    fn close_only_dismisses_the_current_reminder() {
        let dir = superpowers_dir("app-history-notice-close");
        let (ctx, mut app) = history_app(&dir, None);
        app.open_demo();
        frame(&mut app, &ctx, vec![]);
        let id = app.active_session.unwrap();
        app.actions.push(AppAction::DismissNotice(id));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(strips(&app), [None]);
        app.actions.push(AppAction::Disconnect(id));
        frame(&mut app, &ctx, vec![]);
        app.actions.push(AppAction::Reconnect(id));
        frame(&mut app, &ctx, vec![]);
        assert_eq!(strips(&app), [Some(StripNotice::HistoryRecording)]);
        join_history(&mut app);
        let tab = &mut app.sessions.get_mut(id).unwrap().tab;
        tab.flush_history()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let sessions = app
            .history_store
            .clone()
            .unwrap()
            .sessions(&Default::default(), 0, 10)
            .unwrap();
        assert_eq!(sessions.len(), 2, "a reconnect is a new history session");
        assert!(sessions.iter().any(|s| s.ended_at.is_some()));
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// View > Session history opens the window (non-modal: the session keeps running) and it
    /// reads what open sessions recorded, flushed first.
    #[test]
    fn the_history_window_opens_from_the_view_menu_and_reads_open_sessions() {
        let dir = superpowers_dir("app-history-window");
        let (ctx, mut app) = history_app(&dir, None);
        assert!(app.menu_state().has_history);
        app.open_demo();
        frame(&mut app, &ctx, vec![]);
        let id = app.active_session.unwrap();
        let tab = &mut app.sessions.get_mut(id).unwrap().tab;
        tab.input = "north".into();
        tab.submit();
        app.run_command(&ctx, Command::SessionHistory);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            frame(&mut app, &ctx, vec![]);
            let window = app.history_window.as_ref().expect("the window is open");
            if !window.model.working() && !window.model.sessions.is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "the window never read its sessions");
            std::thread::sleep(Duration::from_millis(10));
        }
        let window = app.history_window.as_mut().unwrap();
        assert_eq!(window.model.sessions[0].world_key, "demo");
        window.model.query = "north".into();
        window.model.filters_changed();
        window.model.refresh();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.history_window.as_ref().unwrap().model.working() {
            frame(&mut app, &ctx, vec![]);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let model = &app.history_window.as_ref().unwrap().model;
        assert!(
            model.results.iter().any(|h| h.entry.text == "north"),
            "{:?}",
            model.results
        );
        // Play goes on while it is open.
        assert!(app.sessions.get(id).unwrap().tab.is_connected());
        let texts = drawn_text(&mut app, &ctx);
        assert!(texts.iter().any(|x| x == t(S::HistoryLocalNotice)));
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Search / refresh in an open History window flushes the open sessions first: a command
    /// sent after the window opened is found at once, not after the recorder's two second batch.
    #[test]
    fn a_history_search_flushes_open_sessions_first() {
        let dir = superpowers_dir("app-history-search-flush");
        let (ctx, mut app) = history_app(&dir, None);
        app.open_demo();
        frame(&mut app, &ctx, vec![]);
        let id = app.active_session.unwrap();
        app.run_command(&ctx, Command::SessionHistory);
        let settle = |app: &mut WandurApp| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                frame(app, &ctx, vec![]);
                if !app.history_window.as_ref().unwrap().model.working() {
                    break;
                }
                assert!(Instant::now() < deadline, "the window never finished reading");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        settle(&mut app);
        // Sent while the window is open: still in the recorder's batch.
        let tab = &mut app.sessions.get_mut(id).unwrap().tab;
        tab.input = "south".into();
        tab.submit();
        frame(&mut app, &ctx, vec![]);
        let window = app.history_window.as_mut().unwrap();
        window.model.query = "south".into();
        window.model.filters_changed();
        window.request_search();
        settle(&mut app);
        let model = &app.history_window.as_ref().unwrap().model;
        assert!(
            model.results.iter().any(|h| h.entry.text == "south"),
            "{:?}",
            model.results
        );
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Retention removes expired entries at start and after a saved change, by the history
    /// clock; Forever removes nothing.
    #[test]
    fn retention_runs_at_start_and_after_a_saved_change() {
        use wandur_core::history::{EntryKind, HistoryEntry, HistorySession, SqliteHistoryStore};
        let dir = superpowers_dir("app-history-retention");
        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        let days = |n: u64| now - Duration::from_secs(n * 86_400);
        let store = SqliteHistoryStore::new(Database::open(&dir).unwrap().0);
        let add = |id: &str, age: u64| {
            let session = HistorySession {
                id: id.into(),
                world_key: "demo".into(),
                world_name: "Demo".into(),
                character_name: String::new(),
                started_at: days(age),
                ended_at: Some(days(age)),
            };
            let entry = HistoryEntry {
                sequence: 1,
                at: days(age),
                kind: EntryKind::Received,
                text: format!("{id}marker"),
            };
            store.append(&session, &[entry]).unwrap();
        };
        add("forty", 40);
        add("twenty", 20);
        let clock: Clock = Arc::new(move || now);
        let (ctx, mut app) = history_app(&dir, Some(clock));
        join_history(&mut app);
        let found = |q: &str| store.search(q, &Default::default(), 0, 10).unwrap().len();
        assert_eq!(
            (found("fortymarker"), found("twentymarker")),
            (0, 1),
            "30 days at start"
        );
        add("hundred", 100);
        add("sixty", 60);
        app.settings.history_retention_days = 90;
        app.settings_changed(&ctx);
        join_history(&mut app);
        assert_eq!(
            (found("hundredmarker"), found("sixtymarker")),
            (0, 1),
            "90 days after the change"
        );
        add("thousand", 1000);
        app.settings.history_retention_days = 0;
        app.settings_changed(&ctx);
        join_history(&mut app);
        assert_eq!(found("thousandmarker"), 1, "forever keeps everything");
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The C# UpdateNoticeTests' fake source: answers `version`, or fails.
    #[derive(Default)]
    struct FakeUpdates {
        version: std::sync::Mutex<String>,
        fail: std::sync::atomic::AtomicBool,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeUpdates {
        fn new(version: &str) -> Arc<Self> {
            let fake = Self::default();
            *fake.version.lock().unwrap() = version.into();
            Arc::new(fake)
        }
        fn set(&self, version: &str, fail: bool) {
            *self.version.lock().unwrap() = version.into();
            self.fail.store(fail, std::sync::atomic::Ordering::SeqCst);
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl wandur_core::updates::UpdateSource for FakeUpdates {
        fn latest(&self) -> Result<wandur_core::updates::UpdateInfo, String> {
            use std::sync::atomic::Ordering;
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err("offline".into());
            }
            let version = self.version.lock().unwrap().clone();
            Ok(wandur_core::updates::UpdateInfo {
                notes: Some(format!(
                    "https://github.com/Last-Mile-Studio/wandur/releases/tag/v{version}"
                )),
                version,
                page: wandur_core::updates::DOWNLOADS_PAGE.into(),
            })
        }
    }

    type Moment = Arc<std::sync::Mutex<std::time::SystemTime>>;

    fn update_app(
        dir: &std::path::Path,
        source: &Arc<FakeUpdates>,
        version: &str,
        clock: Option<&Moment>,
    ) -> (egui::Context, WandurApp) {
        let ctx = egui::Context::default();
        let update_clock = clock.map(|c| {
            let c = Arc::clone(c);
            Arc::new(move || *c.lock().unwrap()) as wandur_core::updates::Clock
        });
        let app = WandurApp::new(
            &ctx,
            Options {
                update_source: Some(Arc::clone(source) as Arc<dyn wandur_core::updates::UpdateSource>),
                update_version: Some(version.into()),
                update_clock,
                launcher: Some(Arc::new(|_: &str| true)),
                ..offline_options(dir)
            },
        );
        (ctx, app)
    }

    fn info(heading: &str, message: &str) -> Option<Dialog> {
        Some(Dialog::Information {
            heading: heading.into(),
            message: message.into(),
        })
    }

    /// UpdateNoticeTests.CheckForUpdatesSaysTheLatestVersionShowsTheNoticeOrSaysWandurNetCouldNotBeReached.
    #[test]
    fn check_for_updates_says_the_latest_version_shows_the_notice_or_says_it_could_not_reach_the_site() {
        let dir = superpowers_dir("app-updates-menu");
        let source = FakeUpdates::new("0.1.5");
        let (ctx, mut app) = update_app(&dir, &source, "0.1.5", None);
        frame(&mut app, &ctx, vec![]);

        app.check_for_updates_and_wait(&ctx);
        let (offered, visible, dialog) = app.update_state();
        assert_eq!(dialog, info("You have the latest version, 0.1.5.", ""));
        assert!(offered.is_none() && !visible);
        app.dialog = None;

        source.set("0.1.5", true);
        app.check_for_updates_and_wait(&ctx);
        let (_, visible, dialog) = app.update_state();
        assert_eq!(dialog, info(t(S::UpdateUnreachable), t(S::UpdateUnreachableHint)));
        assert_eq!(t(S::UpdateUnreachable), "Could not reach wandur.net.");
        assert!(!visible);
        app.dialog = None;

        source.set("0.1.6", false);
        app.check_for_updates_and_wait(&ctx);
        let (offered, visible, dialog) = app.update_state();
        assert_eq!(dialog, None, "the notice is the answer");
        assert_eq!(offered.as_deref(), Some("0.1.6"));
        assert!(visible);
        let texts = drawn_text(&mut app, &ctx);
        assert!(
            texts.iter().any(|x| x == "Wandur Mud Client 0.1.6 is available."),
            "{texts:?}"
        );
        assert!(texts.iter().any(|x| x == t(S::UpdateDownload)));
        assert!(texts.iter().any(|x| x == t(S::UpdateSkipVersion)));
        assert_eq!(source.calls(), 3);
        // Each check is remembered, so a restart within the day does not ask again.
        assert_eq!(
            app.settings
                .last_update_check
                .as_ref()
                .and_then(|r| r.version.as_deref()),
            Some("0.1.6")
        );
        // The menu item is there and enabled.
        assert!(
            menus::all_items(&menus::menus(app.platform, &app.menu_state()))
                .iter()
                .any(|i| i.command == Command::CheckForUpdates && i.enabled)
        );
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// UpdateNoticeTests.ABuildFromSourceSaysChecksAreOffAndNeverAsks.
    #[test]
    fn a_build_from_source_says_checks_are_off_and_never_asks() {
        let dir = superpowers_dir("app-updates-dev");
        let source = FakeUpdates::new("0.1.6");
        let (ctx, mut app) = update_app(&dir, &source, "0.0.0-dev", None);
        app.run_command(&ctx, Command::CheckForUpdates);
        assert_eq!(
            app.update_state().2,
            info(&tf(S::UpdateChecksOffInSourceBuild, &[&"0.0.0-dev"]), "")
        );
        app.automatic_update_check(&ctx);
        assert_eq!(source.calls(), 0);
        assert!(!app.update_state().1);
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// UpdateNoticeTests.TheAutomaticCheckRunsOnceADayAndARestartShowsTheRememberedNoticeWithoutAsking,
    /// and SkippingHidesThatVersionForGoodAndANewerOneShowsAgain.
    #[test]
    fn the_automatic_check_runs_once_a_day_survives_a_restart_and_skipping_hides_that_version() {
        use std::time::UNIX_EPOCH;
        let dir = superpowers_dir("app-updates-daily");
        let source = FakeUpdates::new("0.1.6");
        let at = |hours: u64| UNIX_EPOCH + Duration::from_secs(1_791_028_800 + hours * 3600);
        let clock: Moment = Arc::new(std::sync::Mutex::new(at(0)));
        let (ctx, mut app) = update_app(&dir, &source, "0.1.5", Some(&clock));
        app.automatic_update_check(&ctx);
        app.automatic_update_check(&ctx);
        assert_eq!(source.calls(), 1);
        assert!(app.update_state().1);
        app.on_exit();
        drop(app);

        *clock.lock().unwrap() = at(20);
        let (ctx, mut app) = update_app(&dir, &source, "0.1.5", Some(&clock));
        assert!(app.update_state().1, "from the saved result");
        app.automatic_update_check(&ctx);
        assert_eq!(source.calls(), 1);

        *clock.lock().unwrap() = at(24);
        source.set("0.1.6", true);
        app.automatic_update_check(&ctx);
        assert_eq!(source.calls(), 2, "due again; the failure is silent");
        assert_eq!(app.update_state().2, None);
        assert!(app.update_state().1);
        app.automatic_update_check(&ctx);
        assert_eq!(source.calls(), 2, "no retry");

        // Skip this version: hidden for good, also after a restart and a day.
        source.set("0.1.6", false);
        frame(&mut app, &ctx, vec![]);
        let skip = drawn_rect(&mut app, &ctx, t(S::UpdateSkipVersion));
        click_at(&mut app, &ctx, skip);
        assert!(!app.update_state().1);
        assert_eq!(app.settings.skipped_update_version.as_deref(), Some("0.1.6"));
        app.on_exit();
        drop(app);
        let (ctx, mut app) = update_app(&dir, &source, "0.1.5", Some(&clock));
        assert!(!app.update_state().1);
        *clock.lock().unwrap() = at(48);
        app.automatic_update_check(&ctx);
        assert!(!app.update_state().1);

        source.set("0.1.7", false);
        *clock.lock().unwrap() = at(72);
        app.automatic_update_check(&ctx);
        let (offered, visible, _) = app.update_state();
        assert_eq!(offered.as_deref(), Some("0.1.7"));
        assert!(visible);
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// UpdateNoticeTests.TheNoticeHidesDuringPrivateInputAndNeverTakesFocus: hidden while the
    /// active session's input is private; the × hides it for this run only; Download opens
    /// the downloads page and the command box keeps the focus.
    #[test]
    fn the_notice_hides_during_private_input_and_the_cross_hides_it_for_this_run() {
        let dir = superpowers_dir("app-updates-private");
        let source = FakeUpdates::new("0.1.6");
        let opened: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let ctx = egui::Context::default();
        let log = Arc::clone(&opened);
        let mut app = WandurApp::new(
            &ctx,
            Options {
                update_source: Some(Arc::clone(&source) as Arc<dyn wandur_core::updates::UpdateSource>),
                update_version: Some("0.1.5".into()),
                launcher: Some(Arc::new(move |url: &str| {
                    log.lock().unwrap().push(url.to_string());
                    true
                })),
                demo: true,
                ..offline_options(&dir)
            },
        );
        frame(&mut app, &ctx, vec![]);
        app.check_for_updates_and_wait(&ctx);
        assert!(app.update_state().1);
        app.run_command(&ctx, Command::PrivateInput);
        assert!(app.active_private());
        assert!(!app.update_state().1);
        let texts = drawn_text(&mut app, &ctx);
        assert!(!texts.iter().any(|x| x.contains("is available")));
        app.run_command(&ctx, Command::PrivateInput);
        assert!(app.update_state().1);

        frame(&mut app, &ctx, vec![]);
        let focused = ctx.memory(|m| m.focused());
        let download = drawn_rect(&mut app, &ctx, t(S::UpdateDownload));
        click_at(&mut app, &ctx, download);
        assert_eq!(*opened.lock().unwrap(), [wandur_core::updates::DOWNLOADS_PAGE]);
        assert_eq!(ctx.memory(|m| m.focused()), focused, "the focus stays where it was");

        let cross = drawn_rect(&mut app, &ctx, "×");
        click_at(&mut app, &ctx, cross);
        assert!(!app.update_state().1);
        assert_eq!(app.settings.skipped_update_version, None);
        app.on_exit();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// TheNoticeReadsInEveryLanguage: the strip's text and Skip this version in each language.
    #[test]
    fn the_notice_reads_in_every_language() {
        for language in Language::ALL {
            let dir = superpowers_dir(&format!("app-updates-{}", language.code()));
            let source = FakeUpdates::new("0.1.6");
            let ctx = egui::Context::default();
            let mut app = WandurApp::new(
                &ctx,
                Options {
                    update_source: Some(Arc::clone(&source) as Arc<dyn wandur_core::updates::UpdateSource>),
                    update_version: Some("0.1.5".into()),
                    language: Some(language),
                    ..offline_options(&dir)
                },
            );
            app.check_for_updates_and_wait(&ctx);
            let texts = drawn_text(&mut app, &ctx);
            let available = wandur_core::l10n::text_in(language, S::UpdateAvailable).replace("{0}", "0.1.6");
            let skip = wandur_core::l10n::text_in(language, S::UpdateSkipVersion).to_string();
            assert!(texts.contains(&available), "{language:?}: {texts:?}");
            assert!(texts.contains(&skip), "{language:?}");
            app.on_exit();
            drop(app);
            let _ = std::fs::remove_dir_all(&dir);
        }
        wandur_core::l10n::override_thread(None);
    }

    /// Where `text` was drawn last frame (its centre).
    fn drawn_rect(app: &mut WandurApp, ctx: &egui::Context, text: &str) -> egui::Pos2 {
        fn walk(shape: &egui::Shape, text: &str, out: &mut Option<egui::Pos2>) {
            match shape {
                // The topmost one (the update strip sits right under the menu bar).
                egui::Shape::Text(t) if t.galley.text() == text => {
                    let at = t.pos + t.galley.rect.center().to_vec2();
                    if out.is_none_or(|o| at.y < o.y) {
                        *out = Some(at);
                    }
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
                _ => {}
            }
        }
        frame(app, ctx, vec![]);
        let mut eframe_frame = eframe::Frame::_new_kittest();
        let input = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx(), &mut eframe_frame);
            app.ui(ui, &mut eframe_frame);
        });
        out.textures_delta.clear();
        let mut found = None;
        for clipped in &out.shapes {
            walk(&clipped.shape, text, &mut found);
        }
        found.unwrap_or_else(|| panic!("{text} was not drawn"))
    }

    fn click_at(app: &mut WandurApp, ctx: &egui::Context, pos: egui::Pos2) {
        for events in click(pos) {
            frame(app, ctx, events);
        }
        frame(app, ctx, vec![]);
    }

    /// A headless harness over the whole app (embedded viewports, so the world editor is a
    /// window inside the main one), with the AccessKit tree to find controls by name.
    fn app_harness(dir: &std::path::Path) -> egui_kittest::Harness<'static, Option<WandurApp>> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(1200.0, 800.0))
            .build_ui_state(
                |ui, app: &mut Option<WandurApp>| {
                    if let Some(app) = app {
                        let mut eframe_frame = eframe::Frame::_new_kittest();
                        app.logic(ui.ctx(), &mut eframe_frame);
                        app.ui(ui, &mut eframe_frame);
                    }
                },
                None,
            );
        let app = WandurApp::new(&harness.ctx, offline_options(dir));
        *harness.state_mut() = Some(app);
        harness.run_steps(2);
        harness
    }

    /// A saved world whose library is `entries`, open in the world editor's Scripts section.
    fn open_scripts(
        harness: &mut egui_kittest::Harness<'static, Option<WandurApp>>,
        entries: Vec<LibraryEntry>,
    ) -> String {
        let app = harness.state_mut().as_mut().unwrap();
        app.settings.enable_lua_scripts = true;
        let world = SavedWorld {
            name: "Typing Hall".into(),
            host: "127.0.0.1".into(),
            port: 4000,
            ..SavedWorld::default()
        };
        let i = app.save_world(None, world);
        let id = app.settings.worlds[i].world_id.clone();
        let before = app.library(&id);
        app.save_library(&id, LibraryChange { before, after: entries });
        app.db_writer.as_ref().unwrap().flush();
        app.edit_world(Some(i));
        app.form.as_mut().unwrap().section = crate::world_form::Section::Scripts;
        harness.run_steps(3);
        id
    }

    fn select_script(harness: &mut egui_kittest::Harness<'static, Option<WandurApp>>, name: &str) {
        use egui_kittest::kittest::Queryable;
        harness.get_by_label(name).click();
        harness.run_steps(3);
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        assert_eq!(form.macros.selected_script_entry().map(|e| e.name.as_str()), Some(name));
    }

    /// Click into the code editor and type; returns the selected script's source after.
    fn type_in_editor(harness: &mut egui_kittest::Harness<'static, Option<WandurApp>>, typed: &str) -> String {
        use egui_kittest::kittest::Queryable;
        // Near the top left of the text (a long script's text runs past the visible part, so
        // its middle may be out of view).
        let at = harness.get_by_label(t(S::ScriptEditorTitle)).rect().min + vec2(60.0, 16.0);
        for pressed in [true, false] {
            harness.event(Event::PointerMoved(at));
            harness.event(Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
            harness.run_steps(1);
        }
        harness.run_steps(1);
        harness.get_by_label(t(S::ScriptEditorTitle)).type_text(typed);
        harness.run_steps(2);
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        form.macros.selected_script_entry().unwrap().source.clone()
    }

    /// The owner's report: scripts could not be edited in the world editor. Clicking into the
    /// code editor and typing changes an ordinary JavaScript script, a Lua script and scripts
    /// imported from Mudlet (none of them is a pack script), and Save world writes the edits.
    #[test]
    fn scripts_typed_in_the_world_editor_change_and_save() {
        use egui_kittest::kittest::Queryable;
        let dir = superpowers_dir("app-script-typing");
        let mut harness = app_harness(&dir);
        let mut js = wandur_core::db::scripts::starter();
        js.name = "Typed JavaScript".into();
        let (mudlet, _) = crate::scene::lantern_mudlet_library();
        let lua = mudlet
            .iter()
            .find(|e| e.is_lua() && e.import.is_none())
            .cloned()
            .expect("the fixture's Lua script");
        let imported: Vec<LibraryEntry> = mudlet
            .iter()
            .filter(|e| e.import.is_some() && !e.is_macro())
            .cloned()
            .collect();
        assert!(!imported.is_empty(), "the Mudlet fixture imports scripts");
        assert!(
            imported.iter().all(|e| !e.is_pack()),
            "Mudlet imports are not pack scripts"
        );
        let mut entries = vec![js.clone(), lua.clone()];
        entries.extend(imported.iter().cloned());
        let world_id = open_scripts(&mut harness, entries);

        select_script(&mut harness, "Typed JavaScript");
        let after = type_in_editor(&mut harness, "/*js*/");
        assert!(
            after.contains("/*js*/"),
            "JavaScript typing reached the source: {after}"
        );
        select_script(&mut harness, &lua.name);
        let after = type_in_editor(&mut harness, "--lua ");
        assert!(after.contains("--lua "), "Lua typing reached the source: {after}");
        for entry in &imported {
            select_script(&mut harness, &entry.name);
            let after = type_in_editor(&mut harness, "--mudlet ");
            assert!(after.contains("--mudlet "), "typing reached {}: {after}", entry.name);
        }

        harness.get_by_label(t(S::SaveWorld)).click();
        harness.run_steps(3);
        let app = harness.state_mut().as_mut().unwrap();
        assert!(app.form.is_none(), "Save world closed the editor");
        app.db_writer.as_ref().unwrap().flush();
        let saved = app
            .db
            .as_ref()
            .unwrap()
            .read(|c| wandur_core::db::scripts::load(c, &world_id))
            .unwrap();
        let source = |id: &str| saved.iter().find(|e| e.id == id).unwrap().source.clone();
        assert!(source(&js.id).contains("/*js*/"));
        assert!(source(&lua.id).contains("--lua "));
        for entry in &imported {
            assert!(source(&entry.id).contains("--mudlet "), "{} saved", entry.name);
        }
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pack script is read only, and says so: the notice with Duplicate above a read-only
    /// editor; typing in it changes nothing and asks whether to make an editable copy; Cancel
    /// drops the question; Make copy selects an ordinary copy (of the formatted text it was
    /// shown as) with the caret where it was and the keystroke typed into it.
    #[test]
    fn typing_in_a_pack_script_offers_an_editable_copy() {
        use egui_kittest::kittest::{NodeT as _, Queryable};
        let dir = superpowers_dir("app-pack-copy");
        let mut harness = app_harness(&dir);
        let pack = crate::scene::lantern_pack_script();
        let original = pack.source.clone();
        open_scripts(&mut harness, vec![pack.clone()]);
        select_script(&mut harness, &pack.name);
        harness.get_by_label(t(S::ScriptPackReadOnlyNotice));
        assert!(
            harness
                .get_by_label(t(S::ScriptEditorTitle))
                .accesskit_node()
                .is_read_only()
        );
        assert!(harness.query_by_label(t(S::ScriptPackEditPrompt)).is_none());

        // Typing: nothing changes, the question shows; Cancel drops it.
        assert_eq!(type_in_editor(&mut harness, "Q"), original);
        harness.get_by_label(t(S::ScriptPackEditPrompt));
        // (The prompt's Cancel, not the footer's.)
        harness
            .get_all_by_label(t(S::Cancel))
            .min_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y))
            .unwrap()
            .click();
        harness.run_steps(2);
        assert!(harness.query_by_label(t(S::ScriptPackEditPrompt)).is_none());
        assert!(
            harness.state().as_ref().unwrap().form.is_some(),
            "the editor stayed open"
        );

        // Again, then Make copy: the copy is selected, editable, and has the keystroke.
        assert_eq!(type_in_editor(&mut harness, "Q"), original);
        let caret = harness
            .state()
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .macros
            .editor
            .caret;
        harness.get_by_label(t(S::ScriptPackMakeCopy)).click();
        harness.run_steps(4);
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        let copy = form.macros.selected_script_entry().unwrap();
        assert_eq!(copy.name, tf(S::ScriptDuplicateName, &[&pack.name]));
        assert!(!copy.is_pack(), "the copy is an ordinary script");
        assert_ne!(copy.id, pack.id);
        // The copy starts from the formatted text the pack script was shown as.
        let shown = crate::script_editor::shown_text(&original, false);
        assert_ne!(shown, original, "the fixture is shown formatted");
        let at = shown.char_indices().nth(caret).map_or(shown.len(), |(i, _)| i);
        let mut expected = shown.clone();
        expected.insert(at, 'Q');
        assert_eq!(copy.source, expected, "the keystroke went into the copy at the caret");
        assert!(form.macros.pack_prompt.is_none());
        assert!(
            !harness
                .get_by_label(t(S::ScriptEditorTitle))
                .accesskit_node()
                .is_read_only()
        );
        // The copy goes on taking keys.
        let after = type_in_editor(&mut harness, "/*mine*/");
        assert!(after.contains("/*mine*/"), "{after}");
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        let kept = form.macros.entries.iter().find(|e| e.id == pack.id).unwrap();
        assert_eq!(kept.source, original, "the pack script itself is untouched");

        // The notice's Duplicate makes a copy without asking.
        select_script(&mut harness, &pack.name);
        let before = harness
            .state()
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .macros
            .entries
            .len();
        harness.get_by_label(t(S::ScriptDuplicate)).click();
        harness.run_steps(2);
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        assert_eq!(form.macros.entries.len(), before + 1);
        assert!(!form.macros.selected_script_entry().unwrap().is_pack());
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An ordinary script written on one line, for the formatting tests.
    fn dense_script() -> LibraryEntry {
        LibraryEntry {
            name: "Ferry watch (mine)".into(),
            source: crate::scene::FERRY_WATCH_DENSE.into(),
            ..wandur_core::db::scripts::starter()
        }
    }

    /// Every script is shown formatted when it is selected, and viewing changes nothing: the
    /// draft keeps the stored source, nothing is unsaved, and Cancel closes without asking. A
    /// script that does not parse and a Lua script are shown as stored; a pack script is shown
    /// formatted and read only.
    #[test]
    fn scripts_are_shown_formatted_and_viewing_changes_nothing() {
        use egui_kittest::kittest::{NodeT as _, Queryable};
        let dir = superpowers_dir("app-script-formatted");
        let mut harness = app_harness(&dir);
        let dense = dense_script();
        let broken = LibraryEntry {
            name: "Half written".into(),
            source: crate::scene::HALF_WRITTEN.into(),
            ..wandur_core::db::scripts::starter()
        };
        let lua = LibraryEntry {
            name: "Greeter (Lua)".into(),
            source: "mud.echo(   'lua is shown as stored'   )".into(),
            language: wandur_core::scripting::Language::Lua,
            ..wandur_core::db::scripts::starter()
        };
        let pack = crate::scene::lantern_dense_pack_script();
        open_scripts(
            &mut harness,
            vec![dense.clone(), broken.clone(), lua.clone(), pack.clone()],
        );
        let formatted = wandur_format::javascript(crate::scene::FERRY_WATCH_DENSE).unwrap();
        assert!(formatted.lines().count() > 20, "{formatted}");
        for (entry, expected) in [
            (&dense, formatted.as_str()),
            (&broken, broken.source.as_str()),
            (&lua, lua.source.as_str()),
            (&pack, formatted.as_str()),
        ] {
            select_script(&mut harness, &entry.name);
            let editor = harness.get_by_label(t(S::ScriptEditorTitle));
            assert_eq!(
                editor.accesskit_node().value().as_deref(),
                Some(expected),
                "{}",
                entry.name
            );
            let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
            assert_eq!(
                form.macros.selected_script_entry().unwrap().source,
                entry.source,
                "{}: viewing kept the stored source",
                entry.name
            );
            assert!(!form.macros.is_unsaved(&entry.id), "{}", entry.name);
            assert!(!form.has_unsaved_changes(), "{}", entry.name);
        }
        // The pack script stays read only.
        assert!(
            harness
                .get_by_label(t(S::ScriptEditorTitle))
                .accesskit_node()
                .is_read_only()
        );
        // Cancel closes at once: there is nothing to keep.
        harness
            .get_all_by_label(t(S::Cancel))
            .max_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y))
            .unwrap()
            .click();
        harness.run_steps(3);
        assert!(
            harness.state().as_ref().unwrap().form.is_none(),
            "closed without asking"
        );
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Once the person edits a script shown formatted, the draft holds the formatted text with
    /// the edit, and Save world writes it.
    #[test]
    fn an_edit_to_a_formatted_script_saves_the_formatted_text() {
        use egui_kittest::kittest::Queryable;
        let dir = superpowers_dir("app-script-formatted-edit");
        let mut harness = app_harness(&dir);
        let dense = dense_script();
        let world_id = open_scripts(&mut harness, vec![dense.clone()]);
        select_script(&mut harness, &dense.name);
        let after = type_in_editor(&mut harness, "/*mine*/");
        let formatted = wandur_format::javascript(&dense.source).unwrap();
        assert_eq!(
            after.replacen("/*mine*/", "", 1),
            formatted,
            "the edit went into the formatted text"
        );
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        assert!(form.has_unsaved_changes());
        harness.get_by_label(t(S::SaveWorld)).click();
        harness.run_steps(3);
        let app = harness.state_mut().as_mut().unwrap();
        assert!(app.form.is_none(), "Save world closed the editor");
        app.db_writer.as_ref().unwrap().flush();
        let saved = app
            .db
            .as_ref()
            .unwrap()
            .read(|c| wandur_core::db::scripts::load(c, &world_id))
            .unwrap();
        assert_eq!(saved.iter().find(|e| e.id == dense.id).unwrap().source, after);
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The formatter is not loaded at start, nor by the world editor's other sections; the first
    /// script shown loads it, and a script shown again comes from the cache. (The formatter's
    /// state is per process and other tests use it, so the check runs in a child process.)
    #[test]
    fn the_formatter_loads_on_the_first_script_shown() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "app::tests::formatter_lazy_child",
                "--test-threads=1",
            ])
            .env("WANDUR_FORMAT_LAZY_CHILD", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "run in its own process by the_formatter_loads_on_the_first_script_shown"]
    fn formatter_lazy_child() {
        if std::env::var_os("WANDUR_FORMAT_LAZY_CHILD").is_none() {
            return;
        }
        assert!(!wandur_format::is_loaded());
        let dir = superpowers_dir("app-format-lazy");
        let mut harness = app_harness(&dir);
        harness.run_steps(5);
        assert!(!wandur_format::is_loaded(), "not loaded at start");
        let app = harness.state_mut().as_mut().unwrap();
        let world = SavedWorld {
            name: "Lazy Hall".into(),
            host: "127.0.0.1".into(),
            port: 4000,
            ..SavedWorld::default()
        };
        let i = app.save_world(None, world);
        let id = app.settings.worlds[i].world_id.clone();
        let before = app.library(&id);
        let first = dense_script();
        let second = LibraryEntry {
            name: "Second".into(),
            ..wandur_core::db::scripts::starter()
        };
        app.save_library(
            &id,
            LibraryChange {
                before,
                after: vec![first.clone(), second.clone()],
            },
        );
        app.db_writer.as_ref().unwrap().flush();
        app.edit_world(Some(i));
        harness.run_steps(3);
        assert!(!wandur_format::is_loaded(), "not loaded by the Connection section");
        harness.state_mut().as_mut().unwrap().form.as_mut().unwrap().section = crate::world_form::Section::Scripts;
        harness.run_steps(3);
        assert!(wandur_format::is_loaded(), "loaded by the first script shown");
        assert_eq!(wandur_format::shared_runs(), 1);
        select_script(&mut harness, &second.name);
        select_script(&mut harness, &first.name);
        assert_eq!(wandur_format::shared_runs(), 2, "one run per distinct source");
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The world editor is a window that can be moved and resized (here the fallback inside
    /// the main window): dragging its title bar moves it, dragging its corner resizes it and the
    /// script editor follows; the main window under it takes no clicks; Escape cancels.
    #[test]
    fn the_world_editor_window_moves_resizes_and_reflows() {
        use egui_kittest::kittest::Queryable;
        let dir = superpowers_dir("app-editor-window");
        let mut harness = app_harness(&dir);
        open_scripts(&mut harness, vec![wandur_core::db::scripts::starter()]);
        let window = || {
            egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new(("dialog-window", crate::world_form::WINDOW_ID)),
            )
        };
        let area = |h: &egui_kittest::Harness<'static, Option<WandurApp>>| {
            h.ctx
                .memory(|m| m.area_rect(window().id))
                .expect("the editor window is shown")
        };
        let drag = |h: &mut egui_kittest::Harness<'static, Option<WandurApp>>, from: egui::Pos2, by: egui::Vec2| {
            let steps = 6;
            h.event(Event::PointerMoved(from));
            h.run_steps(1);
            h.event(Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            h.run_steps(1);
            for i in 1..=steps {
                h.event(Event::PointerMoved(from + by * (i as f32 / steps as f32)));
                h.run_steps(1);
            }
            h.event(Event::PointerButton {
                pos: from + by,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
            h.run_steps(3);
        };
        let before = area(&harness);
        let editor_before = harness.get_by_label(t(S::ScriptEditorTitle)).rect();
        // The corner resizes it; the code editor takes the change.
        drag(&mut harness, before.max - vec2(3.0, 3.0), vec2(-200.0, -120.0));
        let resized = area(&harness);
        assert_eq!(resized.min, before.min);
        assert!(
            (resized.width() - (before.width() - 200.0)).abs() < 3.0,
            "{before:?} -> {resized:?}"
        );
        assert!(
            (resized.height() - (before.height() - 120.0)).abs() < 3.0,
            "{before:?} -> {resized:?}"
        );
        let editor_after = harness.get_by_label(t(S::ScriptEditorTitle)).rect();
        assert!(
            (editor_after.width() - (editor_before.width() - 200.0)).abs() < 3.0,
            "{editor_before:?} -> {editor_after:?}"
        );
        // The title bar moves it.
        drag(&mut harness, resized.center_top() + vec2(120.0, 10.0), vec2(60.0, 50.0));
        let moved = area(&harness);
        assert!(
            (moved.min - resized.min - vec2(60.0, 50.0)).length() < 4.0,
            "{resized:?} -> {moved:?}"
        );
        assert_eq!(moved.size(), resized.size());
        let resized = moved;
        // A click on the dimmed main window outside does nothing, and keeps it open.
        let outside = egui::pos2(resized.right() + 30.0, resized.bottom() - 10.0);
        harness.event(Event::PointerMoved(outside));
        harness.event(Event::PointerButton {
            pos: outside,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        harness.event(Event::PointerButton {
            pos: outside,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        harness.run_steps(3);
        assert!(harness.state().as_ref().unwrap().form.is_some());
        assert!(
            !harness
                .ctx
                .memory(|m| m.allows_interaction(egui::LayerId::background())),
            "the main window is under the modal editor"
        );
        // Escape cancels, as before.
        harness.key_press(egui::Key::Escape);
        harness.run_steps(3);
        assert!(harness.state().as_ref().unwrap().form.is_none());
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The app in a kittest harness, with `vault` as its credential store.
    fn login_harness(
        dir: &std::path::Path,
        vault: &Arc<wandur_core::login::MemoryVault>,
    ) -> egui_kittest::Harness<'static, Option<WandurApp>> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(1200.0, 800.0))
            .build_ui_state(
                |ui, app: &mut Option<WandurApp>| {
                    if let Some(app) = app {
                        let mut eframe_frame = eframe::Frame::_new_kittest();
                        app.logic(ui.ctx(), &mut eframe_frame);
                        app.ui(ui, &mut eframe_frame);
                    }
                },
                None,
            );
        let app = WandurApp::new(
            &harness.ctx,
            Options {
                vault: Some(Arc::clone(vault) as Arc<dyn PasswordVault>),
                ..offline_options(dir)
            },
        );
        *harness.state_mut() = Some(app);
        harness.run_steps(2);
        harness
    }

    /// Open saved world `i` in the editor (as the saved worlds list does), wait for the store's
    /// answer about its saved password, and show the Login section.
    fn edit_login(harness: &mut egui_kittest::Harness<'static, Option<WandurApp>>, i: usize) {
        harness
            .state_mut()
            .as_mut()
            .unwrap()
            .actions
            .push(AppAction::EditWorld(i));
        harness.run_steps(2);
        show_login(harness);
    }

    /// Wait for the store's answer about the open editor's saved password, then show the Login
    /// section.
    fn show_login(harness: &mut egui_kittest::Harness<'static, Option<WandurApp>>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let form = harness
                .state()
                .as_ref()
                .unwrap()
                .form
                .as_ref()
                .expect("the editor is open");
            if form.world.password_id.is_none() || form.saved_password_found().is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "the store never answered");
            std::thread::sleep(Duration::from_millis(5));
        }
        harness.state_mut().as_mut().unwrap().form.as_mut().unwrap().section = crate::world_form::Section::Login;
        harness.run_steps(3);
    }

    fn click_save_world(harness: &mut egui_kittest::Harness<'static, Option<WandurApp>>) {
        use egui_kittest::kittest::Queryable;
        harness.get_by_label(t(S::SaveWorld)).click();
        harness.run_steps(3);
    }

    fn form_error(harness: &egui_kittest::Harness<'static, Option<WandurApp>>) -> Option<String> {
        harness
            .state()
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .and_then(|f| f.error().map(str::to_string))
    }

    /// Owner report: a world with a working saved password (auto-login uses it), opened in the
    /// editor and saved with nothing changed, said "Enter a password to save for this server and
    /// username." Here: a world saved with a password opens with "Saved", and Save world with
    /// nothing changed, or only fields outside the login (name, reconnect, channel rules), keeps
    /// the password; auto-login still finds it afterwards.
    #[test]
    fn an_untouched_world_with_a_saved_password_saves_and_keeps_it() {
        // Strings compare in English whatever another test does to the process language.
        wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
        let dir = superpowers_dir("app-login-untouched");
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        let mut harness = login_harness(&dir, &vault);
        let app = harness.state_mut().as_mut().unwrap();
        let world = SavedWorld {
            name: "Legends".into(),
            host: "Legends.Example.org".into(),
            port: 5656,
            username: "Talek".into(),
            auto_login: true,
            ..SavedWorld::default()
        };
        let i = app.save_world_with_login(None, world, "fixture-secret", true).unwrap();
        let edits: [fn(&mut crate::world_form::WorldForm); 4] = [
            |_| {},
            |f| f.world.name = "Legends of the Fixture".into(),
            |f| f.world.auto_reconnect = !f.world.auto_reconnect,
            |f| {
                f.world.channel_rules.push(wandur_core::channels::ChannelRule::new(
                    "ooc",
                    "^OOC (?<speaker>\\w+): (?<text>.*)$",
                    None,
                ))
            },
        ];
        for edit in edits {
            edit_login(&mut harness, i);
            let form = harness.state_mut().as_mut().unwrap().form.as_mut().unwrap();
            assert_eq!(form.password_state(), crate::world_form::PasswordState::Saved);
            edit(form);
            harness.run_steps(2);
            click_save_world(&mut harness);
            assert_eq!(form_error(&harness), None);
            let app = harness.state().as_ref().unwrap();
            assert!(app.form.is_none(), "the editor closed");
            let saved = &app.settings.worlds[i];
            let config = app.login_config(saved).expect("auto-login is still on");
            assert_eq!(vault.read(&config.key).unwrap().as_deref(), Some("fixture-secret"));
            assert_eq!(vault.len(), 1);
        }
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        wandur_core::l10n::override_thread(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The cause of the owner report: a saved username (or host) with spaces around it, as an
    /// older C# import wrote it. Auto-login worked (it sends the name trimmed, and the vault key
    /// is the saved spelling), but Save world trimmed the field and so changed the key. The
    /// editor now keeps the saved spelling when only spaces differ, and the password is kept.
    #[test]
    fn a_saved_username_with_spaces_keeps_its_password_on_save() {
        // Strings compare in English whatever another test does to the process language.
        wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
        use wandur_core::login::{credentials, vault};
        let dir = superpowers_dir("app-login-spaces");
        let store = Arc::new(wandur_core::login::MemoryVault::new());
        let world = SavedWorld {
            world_id: "0123456789abcdef0123456789abcdef".into(),
            name: "Legends of the Jedi".into(),
            host: "legends.example.org".into(),
            port: 5656,
            username: " Talek ".into(),
            password_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01".into()),
            auto_login: true,
            ..SavedWorld::default()
        };
        Settings {
            worlds: vec![world.clone()],
            ..Settings::default()
        }
        .save(&dir)
        .unwrap();
        store.write(&vault::key(&world).unwrap(), "fixture-secret").unwrap();
        // What the editor used to pass: the username trimmed, a different key. Core now keeps the
        // password for it too (spaces around the username do not count; the entry would move).
        let trimmed = SavedWorld {
            username: "Talek".into(),
            ..world.clone()
        };
        assert_ne!(vault::key(&trimmed).unwrap(), vault::key(&world).unwrap());
        assert_eq!(credentials::keep_saved(Some(&world), &trimmed), Ok(()));

        let mut harness = login_harness(&dir, &store);
        edit_login(&mut harness, 0);
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        assert_eq!(form.saved_password_found(), Some(&Ok(true)));
        assert_eq!(form.password_state(), crate::world_form::PasswordState::Saved);
        click_save_world(&mut harness);
        assert_eq!(form_error(&harness), None);
        let app = harness.state().as_ref().unwrap();
        assert!(app.form.is_none());
        let saved = &app.settings.worlds[0];
        assert_eq!(saved.username, " Talek ", "the saved spelling is kept");
        let config = app.login_config(saved).unwrap();
        assert_eq!(config.username, "Talek");
        assert_eq!(store.read(&config.key).unwrap().as_deref(), Some("fixture-secret"));

        // Typing spaces around the name changes nothing either; another name does.
        edit_login(&mut harness, 0);
        let form = harness.state_mut().as_mut().unwrap().form.as_mut().unwrap();
        form.world.username = "Talek".into();
        assert_eq!(form.password_state(), crate::world_form::PasswordState::Saved);
        form.world.username = "Kelat".into();
        assert_eq!(form.password_state(), crate::world_form::PasswordState::Changed);
        click_save_world(&mut harness);
        assert_eq!(form_error(&harness).as_deref(), Some(t(S::PasswordLoginChanged)));
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        wandur_core::l10n::override_thread(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Login section says what a blank password will do as the fields change: the server or
    /// username changed (Save world then says so); a changed port or TLS keeps "Saved"; and back
    /// to Saved when a change is undone.
    #[test]
    fn the_password_hint_follows_the_login_identity() {
        // Strings compare in English whatever another test does to the process language.
        wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
        use crate::world_form::PasswordState;
        let dir = superpowers_dir("app-login-hint");
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        let mut harness = login_harness(&dir, &vault);
        let app = harness.state_mut().as_mut().unwrap();
        let world = SavedWorld {
            name: "Hint".into(),
            host: "hint.example.org".into(),
            username: "player".into(),
            auto_login: true,
            ..SavedWorld::default()
        };
        let i = app.save_world_with_login(None, world, "fixture-secret", true).unwrap();
        let edits: [fn(&mut crate::world_form::WorldForm); 5] = [
            |f| f.world.host = "other.example.org".into(),
            |f| {
                f.world.host = "other.example.org".into();
                f.world.tls = true;
                f.port_text = "4443".into();
            },
            |f| f.world.username = "someone".into(),
            |f| f.world.username = "Player".into(),
            |f| {
                f.world.username = "someone".into();
                f.world.tls = true;
            },
        ];
        for edit in edits {
            edit_login(&mut harness, i);
            let form = harness.state_mut().as_mut().unwrap().form.as_mut().unwrap();
            edit(form);
            assert_eq!(form.password_state(), PasswordState::Changed);
            harness.run_steps(2);
            click_save_world(&mut harness);
            assert_eq!(form_error(&harness).as_deref(), Some(t(S::PasswordLoginChanged)));
            // A new password saves it.
            harness.state_mut().as_mut().unwrap().form.as_mut().unwrap().password = "fixture-other".into();
            harness.run_steps(1);
            let snapshot = harness.state().as_ref().unwrap().settings.worlds[i].clone();
            click_save_world(&mut harness);
            assert_eq!(form_error(&harness), None);
            // Put the world back for the next edit.
            let app = harness.state_mut().as_mut().unwrap();
            app.save_world_with_login(Some(i), snapshot, "fixture-secret", true)
                .unwrap();
        }
        // A changed port or TLS keeps the password: the hint stays Saved, Save world moves it to
        // the new key (written to settings.json at once), and auto-login finds it.
        // Each edit starts from the previous one's result.
        let kept: [fn(&mut crate::world_form::WorldForm); 5] = [
            |f| f.world.tls = true,
            |f| f.port_text = "4001".into(),
            |f| {
                f.world.tls = false;
                f.port_text = "4443".into();
            },
            |f| {
                f.world.tls = true;
                f.port_text = "5000".into();
            },
            |f| f.world.tls = false,
        ];
        for edit in kept {
            edit_login(&mut harness, i);
            let form = harness.state_mut().as_mut().unwrap().form.as_mut().unwrap();
            edit(form);
            assert_eq!(form.password_state(), PasswordState::Saved);
            harness.run_steps(2);
            let before = harness.state().as_ref().unwrap().settings.worlds[i].clone();
            let old_key = wandur_core::login::vault::key(&before).unwrap();
            click_save_world(&mut harness);
            assert_eq!(form_error(&harness), None);
            let app = harness.state().as_ref().unwrap();
            assert!(app.form.is_none(), "the editor closed");
            let saved = &app.settings.worlds[i];
            assert_eq!(saved.password_id, before.password_id);
            let config = app.login_config(saved).expect("auto-login is still on");
            assert_ne!(config.key, old_key);
            assert_eq!(vault.read(&config.key).unwrap().as_deref(), Some("fixture-secret"));
            assert_eq!(vault.read(&old_key).unwrap(), None, "the old entry is gone");
            assert_eq!(vault.len(), 1);
            let (on_disk, _) = Settings::load(&dir, wandur_term::MAX_SCROLLBACK);
            assert_eq!(
                wandur_core::login::vault::key(&on_disk.worlds[i]).unwrap(),
                config.key,
                "settings.json refers to the moved entry"
            );
        }
        // The host compares without case; untouching the change makes it Saved again.
        edit_login(&mut harness, i);
        let form = harness.state_mut().as_mut().unwrap().form.as_mut().unwrap();
        form.world.host = "HINT.example.org".into();
        assert_eq!(form.password_state(), PasswordState::Saved);
        form.world.username = "x".into();
        assert_eq!(form.password_state(), PasswordState::Changed);
        form.world.username = "player".into();
        assert_eq!(form.password_state(), PasswordState::Saved);
        // Not remembering: no saved password to talk about.
        form.set_remember(false);
        assert_eq!(form.password_state(), PasswordState::None);
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        wandur_core::l10n::override_thread(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Worlds from File > Import from Wandur (C#), with the C# file's odd spellings (spaces
    /// around the username, the host in capitals, an older `host:port:True` address): with
    /// passwords, Save world with nothing changed keeps the copied one; without, the editor says
    /// none was found, Save world asks for it, and typing it saves it.
    #[test]
    fn imported_worlds_save_honestly_with_and_without_passwords() {
        // Strings compare in English whatever another test does to the process language.
        wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
        use crate::world_form::PasswordState;
        use wandur_core::import::csharp::secrets::csharp_login_key;
        use wandur_core::import::csharp::{CsharpSource, ImportOptions, import_into_dir};
        let source = superpowers_dir("app-login-import-source");
        std::fs::create_dir_all(&source).unwrap();
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wandur-core/tests/fixtures/csharp-data/main");
        std::fs::copy(fixture.join("wandur.db"), source.join("wandur.db")).unwrap();
        let profile = "11111111-2222-4333-8444-555555555501";
        {
            let conn = rusqlite::Connection::open(source.join("wandur.db")).unwrap();
            conn.execute(
                "UPDATE profiles SET payload = replace(replace(payload, '\"Username\":\"wayfarer\"', '\"Username\":\" Talek \"'),
                     '\"Host\":\"lantern.fixture.example\"', '\"Host\":\"Lantern.Fixture.Example\"') WHERE id = ?1",
                [profile],
            )
            .unwrap();
            let world: String = conn
                .query_row("SELECT world_id FROM profiles WHERE id = ?1", [profile], |r| r.get(0))
                .unwrap();
            conn.execute(
                "INSERT INTO legacy_endpoints(endpoint_key, source_key, world_id) VALUES('lantern.fixture.example:4000', 'lantern.fixture.example:4000:True', ?1)",
                [&world],
            )
            .unwrap();
        }
        let from = wandur_core::login::MemoryVault::new();
        from.write(
            &csharp_login_key(
                profile,
                "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee01",
                "Lantern.Fixture.Example",
                4000,
                false,
                " Talek ",
            ),
            "fixture-secret",
        )
        .unwrap();
        for include_secrets in [true, false] {
            let dir = superpowers_dir(&format!("app-login-import-{include_secrets}"));
            let vault = Arc::new(wandur_core::login::MemoryVault::new());
            let csharp = CsharpSource::open(&source).unwrap();
            let options = ImportOptions {
                dry_run: false,
                include_secrets,
            };
            import_into_dir(&csharp, &dir, options, &from, vault.as_ref(), &|_| {}).unwrap();
            drop(csharp);
            let mut harness = login_harness(&dir, &vault);
            assert_eq!(harness.state().as_ref().unwrap().settings.worlds[0].username, "Talek");
            edit_login(&mut harness, 0);
            let state = harness
                .state()
                .as_ref()
                .unwrap()
                .form
                .as_ref()
                .unwrap()
                .password_state();
            click_save_world(&mut harness);
            if include_secrets {
                assert_eq!(state, PasswordState::Saved);
                assert_eq!(form_error(&harness), None);
                let app = harness.state().as_ref().unwrap();
                let key = app.login_config(&app.settings.worlds[0]).unwrap().key;
                assert_eq!(vault.read(&key).unwrap().as_deref(), Some("fixture-secret"));
            } else {
                assert_eq!(state, PasswordState::Missing);
                assert_eq!(form_error(&harness).as_deref(), Some(t(S::SavedPasswordMissing)));
                harness.state_mut().as_mut().unwrap().form.as_mut().unwrap().password = "typed-secret".into();
                harness.run_steps(1);
                click_save_world(&mut harness);
                assert_eq!(form_error(&harness), None);
                let app = harness.state().as_ref().unwrap();
                let key = app.login_config(&app.settings.worlds[0]).unwrap().key;
                assert_eq!(vault.read(&key).unwrap().as_deref(), Some("typed-secret"));
                // Unticking Save password is the other way out.
                edit_login(&mut harness, 1);
                let form = harness.state_mut().as_mut().unwrap().form.as_mut().unwrap();
                assert_eq!(form.password_state(), PasswordState::Missing);
                form.set_remember(false);
                harness.run_steps(1);
                click_save_world(&mut harness);
                assert_eq!(form_error(&harness), None);
                assert_eq!(harness.state().as_ref().unwrap().settings.worlds[1].password_id, None);
            }
            harness.state_mut().as_mut().unwrap().on_exit();
            drop(harness);
            let _ = std::fs::remove_dir_all(&dir);
        }
        wandur_core::l10n::override_thread(None);
        let _ = std::fs::remove_dir_all(&source);
    }

    /// Switching worlds in the editor's picker opens the other world with its own login: saving
    /// it unchanged keeps its password, and the first one's is untouched.
    #[test]
    fn switching_worlds_in_the_picker_keeps_each_password() {
        // Strings compare in English whatever another test does to the process language.
        wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
        use egui_kittest::kittest::Queryable;
        let dir = superpowers_dir("app-login-switch");
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        let mut harness = login_harness(&dir, &vault);
        let app = harness.state_mut().as_mut().unwrap();
        for (name, user, secret) in [("Alpha Hall", "alpha", "secret-a"), ("Beta Hall", "beta", "secret-b")] {
            let world = SavedWorld {
                name: name.into(),
                host: format!("{user}.example.org"),
                username: user.into(),
                auto_login: true,
                ..SavedWorld::default()
            };
            app.save_world_with_login(None, world, secret, true).unwrap();
        }
        edit_login(&mut harness, 0);
        // The picker shows the world being edited; choose the other one in its list.
        // (The main window's toolbar has a picker by the same name; the editor's is the one
        // showing the world being edited, inside the editor window.)
        let window = harness
            .ctx
            .memory(|m| m.area_rect(egui::Id::new(("dialog-window", crate::world_form::WINDOW_ID))))
            .expect("the editor window is shown");
        harness
            .get_all_by_label(t(S::ChooseAWorld))
            .find(|n| window.contains_rect(n.rect()))
            .expect("the editor's world picker")
            .click();
        harness.run_steps(3);
        harness.get_all_by_label("Beta Hall").last().unwrap().click();
        harness.run_steps(3);
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        assert_eq!(form.index, Some(1), "the picker switched to Beta Hall");
        show_login(&mut harness);
        let form = harness.state().as_ref().unwrap().form.as_ref().unwrap();
        assert_eq!(form.world.username, "beta");
        assert_eq!(form.password_state(), crate::world_form::PasswordState::Saved);
        click_save_world(&mut harness);
        assert_eq!(form_error(&harness), None);
        let app = harness.state().as_ref().unwrap();
        for (i, secret) in [(0, "secret-a"), (1, "secret-b")] {
            let key = app.login_config(&app.settings.worlds[i]).unwrap().key;
            assert_eq!(vault.read(&key).unwrap().as_deref(), Some(secret));
        }
        harness.state_mut().as_mut().unwrap().on_exit();
        drop(harness);
        wandur_core::l10n::override_thread(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The editor's world is found by its id when the list changed while it was open (a world
    /// before it removed): the right world keeps its password and no other is replaced. Saving
    /// Settings does not put back the worlds as they were when it opened.
    #[test]
    fn the_editor_and_settings_do_not_save_over_a_changed_world_list() {
        // Strings compare in English whatever another test does to the process language.
        wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
        let dir = superpowers_dir("app-login-stale");
        let vault = Arc::new(wandur_core::login::MemoryVault::new());
        let mut harness = login_harness(&dir, &vault);
        let app = harness.state_mut().as_mut().unwrap();
        for (name, secret) in [("First", "secret-1"), ("Second", "secret-2")] {
            let world = SavedWorld {
                name: name.into(),
                host: format!("{}.example.org", name.to_lowercase()),
                username: "player".into(),
                ..SavedWorld::default()
            };
            app.save_world_with_login(None, world, secret, true).unwrap();
        }
        edit_login(&mut harness, 1);
        harness.state_mut().as_mut().unwrap().delete_world(0);
        click_save_world(&mut harness);
        assert_eq!(form_error(&harness), None);
        let app = harness.state_mut().as_mut().unwrap();
        assert_eq!(app.settings.worlds.len(), 1);
        assert_eq!(app.settings.worlds[0].name, "Second");
        let key = wandur_core::login::vault::key(&app.settings.worlds[0]).unwrap();
        assert_eq!(vault.read(&key).unwrap().as_deref(), Some("secret-2"));

        // Settings opened, then a world saved with a new password, then Settings saved.
        app.actions.push(AppAction::OpenSettings);
        harness.run_steps(2);
        let app = harness.state_mut().as_mut().unwrap();
        let world = app.settings.worlds[0].clone();
        app.save_world_with_login(Some(0), world, "secret-3", true).unwrap();
        let now = app.settings.worlds[0].clone();
        let draft = app.settings_dialog.as_ref().unwrap().draft.clone();
        let ctx = harness.ctx.clone();
        let app = harness.state_mut().as_mut().unwrap();
        app.settings_dialog = None;
        app.save_preferences_from_dialog(&ctx, draft);
        assert_eq!(app.settings.worlds[0], now, "the new password reference stays");
        let key = wandur_core::login::vault::key(&app.settings.worlds[0]).unwrap();
        assert_eq!(vault.read(&key).unwrap().as_deref(), Some("secret-3"));
        app.on_exit();
        drop(harness);
        wandur_core::l10n::override_thread(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
#[path = "app_tabs/tests.rs"]
mod tabs_tests;

#[cfg(test)]
#[path = "map_import/app/tests.rs"]
mod map_import_tests;

#[cfg(test)]
#[path = "official_map/app/tests.rs"]
mod official_map_tests;
