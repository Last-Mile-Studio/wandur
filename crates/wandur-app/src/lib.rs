//! The Wandur desktop app: egui presentation over `wandur-core`.
//!
//! Module map: [`app`] (frame loop, settings, actions), [`shell`] (tab routing, status bar),
//! [`workspace`], [`layout`] and [`autohide`] (dock tabs, default layout, saved layout, pinned and auto-hidden panels), [`sessions`] and
//! [`session_tab`] (session state), [`session_tabs`] (the tabs over the document area), [`toast`] (the undo toast), [`terminal_view`] (grid drawing, selection, input line),
//! [`workspace_panel`] (Find a MUD, open sessions), [`saved_worlds_panel`] and [`panel_header`] (the Saved worlds panel, panel actions and headers), [`world_form`],
//! [`directory_view`] and [`world_page`] (the world directory), [`artwork`] (bounded, cancellable
//! picture loading), [`channels_view`] and [`mark_channel`] (the Channels panel and the teaching dialog), [`map_view`], [`diagnostics_view`] and [`vitals_view`] (the Diagnostics page and the vitals strip), [`settings_dialog`], [`menus`], [`dialogs`], [`csharp_import`] (File > Import from Wandur (C#)... and `--import-csharp`), [`update_notice`] (the update check's strip and schedule), [`theme`], [`widgets`],
//! [`scene`] (named screens for headless screenshots), [`a11y`] (AccessKit for the transcript), [`fonts`], [`grid_text`] (the grid's text as one mesh), [`pacer`] (output redraw cap), [`traffic_lights`] and [`platform`] (the macOS window buttons on a drawn skin), [`probe`]
//! and [`sysstat`] (opt-in measurement).

pub mod a11y;
#[cfg(feature = "agent")]
pub mod agent_menu;
pub mod agent_session;
#[cfg(feature = "agent")]
pub mod agent_settings;
pub mod app;
pub mod artwork;
pub mod autohide;
pub mod channels_view;
pub mod csharp_import;
pub mod diagnostics_view;
pub mod dialog_window;
pub mod dialogs;
pub mod directory_view;
pub mod dock_drop;
pub mod fonts;
pub mod grid_text;
pub mod history_view;
pub mod layout;
pub mod map_import;
pub mod map_inference;
pub mod map_palette;
pub mod map_view;
pub mod mark_channel;
#[cfg(feature = "agent")]
pub mod markdown_editor;
pub mod menus;
pub mod native_menu;
pub mod official_map;
pub mod pacer;
pub mod panel_header;
pub mod panel_view;
pub mod platform;
pub mod probe;
pub mod saved_worlds_panel;
pub mod scene;
pub mod screenshot;
pub mod script_editor;
pub mod select;
pub mod session_tab;
pub mod session_tabs;
pub mod sessions;
pub mod settings_dialog;
pub mod shell;
pub mod skin;
pub mod sysstat;
pub mod terminal_view;
pub mod theme;
pub mod title_bar;
pub mod toast;
pub mod traffic_lights;
pub mod update_notice;
pub mod vitals_view;
pub mod widgets;
pub mod workspace;
pub mod workspace_panel;
pub mod world_form;
pub mod world_page;

pub use app::{Options, WandurApp};
#[cfg(all(test, feature = "agent"))]
#[path = "agent/tests.rs"]
mod agent_tests;
#[cfg(test)]
#[path = "map_walk/tests.rs"]
mod map_walk_tests;
