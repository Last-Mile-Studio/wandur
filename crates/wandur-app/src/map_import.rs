//! File > Import map... (and the full map's Import with nothing selected): a Wandur map file,
//! Mudlet's JSON map export or a Mudlet Mapping Protocol XML map brought into a world's map
//! ([`wandur_core::map::mudlet`]).
//!
//! The dialog asks which world the map goes into (the active session's by default), reads the
//! file on a worker thread with progress, and shows a summary (counts per kind, what was left
//! out and why) before anything changes. Import applies it as one undoable step: into the
//! session's live map when the world has one open (the undo toast offers it back), else into
//! the world's saved map on a worker thread (the toast offers that back too). A Mudlet `.dat`
//! file gets a note on how to make the JSON export.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::{Align, Layout, RichText};
use wandur_core::db::Database;
use wandur_core::l10n::{S, t, tf};
use wandur_core::map::RoomMapTracker;
use wandur_core::map::mudlet::{self, ImportError, Prepared, Progress, Stage};
use wandur_core::map::store::{MapStore, MapWorld};

use crate::dialogs::{modal, primary_button, secondary_button};
use crate::session_tab::SessionId;
use crate::theme::Theme;

/// Where an import goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A session's live map (and through it the world's saved map).
    Session(SessionId),
    /// A world's saved map, with no session open.
    World(MapWorld),
}

/// A world the person can pick.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldChoice {
    pub label: String,
    pub target: Target,
}

/// What the person asked for this frame.
#[derive(Debug)]
pub enum ImportAction {
    Open,
    Closed,
    /// Pick a file with the system's picker.
    ChooseFile,
    /// Apply the read map to this target.
    Apply(Box<Prepared>, Target),
}

enum Message {
    Progress(Progress),
    Done(Box<Result<Prepared, ImportError>>),
}

/// An import into a world without a session, done on a worker thread: the world's map as a
/// tracker (for Undo), its undo step, and how many items changed.
pub struct Detached {
    pub world: MapWorld,
    pub tracker: RoomMapTracker,
    pub changed: usize,
}

enum State {
    /// No file yet, or the last one could not be read (why).
    Choosing { error: Option<String> },
    Reading {
        messages: Receiver<Message>,
        progress: Option<Progress>,
    },
    /// Read: the summary, waiting for Import.
    Ready(Box<Prepared>),
    /// Importing into a world without a session.
    Applying(Receiver<Result<Detached, String>>),
}

pub struct MapImportDialog {
    pub choices: Vec<WorldChoice>,
    pub chosen: usize,
    file: Option<String>,
    state: State,
    /// A finished import into a world without a session, for the app to take.
    finished: Option<Result<Detached, String>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl MapImportDialog {
    pub fn new(choices: Vec<WorldChoice>, chosen: usize, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let chosen = chosen.min(choices.len().saturating_sub(1));
        Self {
            choices,
            chosen,
            file: None,
            state: State::Choosing { error: None },
            finished: None,
            wake,
        }
    }

    /// Read a file on a worker thread.
    pub fn read_file(&mut self, path: &Path) {
        self.file = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let path = path.to_path_buf();
        let (send, receive) = channel();
        let wake = Arc::clone(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("wandur-map-import".into())
            .spawn(move || {
                let progress = |p: Progress| {
                    let _ = send.send(Message::Progress(p));
                    wake();
                };
                let result = read_path(&path, &progress);
                let _ = send.send(Message::Done(Box::new(result)));
                wake();
            });
        self.state = match spawned {
            Ok(_) => State::Reading {
                messages: receive,
                progress: None,
            },
            Err(e) => State::Choosing {
                error: Some(e.to_string()),
            },
        };
    }

    /// Import into a world without a session, on a worker thread.
    pub fn apply_detached(&mut self, db: Database, world: MapWorld, prepared: Prepared) {
        let (send, receive) = channel();
        let wake = Arc::clone(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("wandur-map-import".into())
            .spawn(move || {
                let _ = send.send(import_detached(&MapStore::new(db), world, &prepared));
                wake();
            });
        if spawned.is_ok() {
            self.state = State::Applying(receive);
        }
    }

    /// Take what the workers sent.
    pub fn poll(&mut self) {
        match &mut self.state {
            State::Reading { messages, progress } => {
                let mut done = None;
                while let Ok(message) = messages.try_recv() {
                    match message {
                        Message::Progress(p) => *progress = Some(p),
                        Message::Done(result) => done = Some(*result),
                    }
                }
                match done {
                    Some(Ok(prepared)) => self.state = State::Ready(Box::new(prepared)),
                    Some(Err(e)) => {
                        self.state = State::Choosing {
                            error: Some(e.to_string()),
                        }
                    }
                    None => {}
                }
            }
            State::Applying(receive) => {
                if let Ok(result) = receive.try_recv() {
                    self.finished = Some(result);
                }
            }
            _ => {}
        }
    }

    /// Whether a worker is still running.
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Reading { .. } | State::Applying(_))
    }

    /// The read map, once ready.
    pub fn prepared(&self) -> Option<&Prepared> {
        match &self.state {
            State::Ready(p) => Some(p),
            _ => None,
        }
    }

    /// Why the last file could not be read.
    pub fn error(&self) -> Option<&str> {
        match &self.state {
            State::Choosing { error } => error.as_deref(),
            _ => None,
        }
    }

    /// A finished import into a world without a session (taken once).
    pub fn take_finished(&mut self) -> Option<Result<Detached, String>> {
        self.finished.take()
    }

    pub fn target(&self) -> Option<&Target> {
        self.choices.get(self.chosen).map(|c| &c.target)
    }

    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme) -> ImportAction {
        self.poll();
        let mut action = ImportAction::Open;
        let busy = self.busy();
        let response = modal(ctx, "map-import", 560.0, theme, |ui| {
            ui.label(RichText::new(t(S::MapImportTitle)).size(24.0).color(theme.text));
            ui.add_space(8.0);
            ui.add(egui::Label::new(RichText::new(t(S::MapImportIntro)).size(13.0).color(theme.muted)).wrap());
            ui.add_space(12.0);
            // The world.
            if self.choices.is_empty() {
                ui.add(egui::Label::new(RichText::new(t(S::MapImportNoWorld)).size(13.0).color(theme.text)).wrap());
            } else {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t(S::MapImportWorld)).size(13.0).color(theme.text));
                    ui.add_enabled_ui(!busy, |ui| {
                        let labels: Vec<String> = self.choices.iter().map(|c| c.label.clone()).collect();
                        crate::select::Select::new("map-import-world", t(S::MapImportWorld))
                            .width(360.0)
                            .show_index(ui, &mut self.chosen, labels.len(), |i| labels[i].as_str());
                    });
                });
            }
            ui.add_space(10.0);
            // The file.
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if secondary_button(ui, t(S::MapImportChooseFile)).clicked() {
                        action = ImportAction::ChooseFile;
                    }
                });
                if let Some(name) = &self.file {
                    ui.add(egui::Label::new(RichText::new(name).size(13.0).color(theme.text)).truncate());
                }
            });
            ui.add_space(10.0);
            match &self.state {
                State::Choosing { error: Some(e) } => {
                    egui::Frame::new()
                        .fill(theme.shell)
                        .corner_radius(4)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(e).size(13.0).color(theme.text))
                                    .wrap()
                                    .selectable(true),
                            );
                        });
                }
                State::Choosing { error: None } => {}
                State::Reading { progress, .. } => {
                    let (label, fraction) = progress_text(*progress);
                    ui.label(RichText::new(label).size(13.0).color(theme.text));
                    match fraction {
                        Some(f) => ui.add(egui::ProgressBar::new(f).desired_width(500.0)),
                        None => ui.spinner(),
                    };
                }
                State::Applying(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(t(S::MapImportApplying)).size(13.0).color(theme.text));
                    });
                }
                State::Ready(prepared) => summary(ui, prepared, theme),
            }
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ready = matches!(self.state, State::Ready(_)) && !self.choices.is_empty();
                let import = ui
                    .add_enabled_ui(ready, |ui| primary_button(ui, t(S::MapImportStart), theme))
                    .inner;
                if import.clicked()
                    && let State::Ready(prepared) = &self.state
                    && let Some(target) = self.target()
                {
                    action = ImportAction::Apply(prepared.clone(), target.clone());
                }
                let cancel = ui
                    .add_enabled_ui(!matches!(self.state, State::Applying(_)), |ui| {
                        secondary_button(ui, t(S::Cancel2))
                    })
                    .inner;
                if cancel.clicked() {
                    action = ImportAction::Closed;
                }
            });
        });
        if response.should_close() && !matches!(self.state, State::Applying(_)) {
            return ImportAction::Closed;
        }
        action
    }
}

/// The progress line and, when known, how far.
fn progress_text(progress: Option<Progress>) -> (String, Option<f32>) {
    match progress {
        Some(p) if p.total > 0 => {
            let key = match p.stage {
                Stage::Labels => S::MapImportLabelsProgress,
                _ => S::MapImportRoomsProgress,
            };
            (tf(key, &[&p.done, &p.total]), Some(p.done as f32 / p.total as f32))
        }
        _ => (t(S::MapImportReading).to_string(), None),
    }
}

/// The summary: what the file holds, what will happen, what was left out.
fn summary(ui: &mut egui::Ui, prepared: &Prepared, theme: &Theme) {
    let s = &prepared.summary;
    let kind = match s.kind {
        mudlet::SourceKind::Wandur => t(S::MapImportKindWandur),
        mudlet::SourceKind::Mudlet => t(S::MapImportKindMudlet),
        mudlet::SourceKind::MudletXml => t(S::MapImportKindMudletXml),
    };
    ui.label(RichText::new(kind).size(15.0).strong().color(theme.text));
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("map-import-summary")
        .max_height(300.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.indent("map-import-counts", |ui| {
                for line in s.lines() {
                    ui.label(RichText::new(line).size(13.0).color(theme.text));
                }
            });
            if s.flipped {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(t(S::MapImportFlipped)).size(12.5).color(theme.muted)).wrap());
            }
            let skipped = s.skipped_lines();
            if !skipped.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(t(S::MapImportLeftOut)).size(13.0).color(theme.text));
                ui.indent("map-import-skipped", |ui| {
                    for line in skipped {
                        ui.add(egui::Label::new(RichText::new(line).size(12.5).color(theme.muted)).wrap());
                    }
                });
            }
        });
    ui.add_space(6.0);
    let note = if prepared.replaces() {
        t(S::MapImportReplaceNote)
    } else {
        t(S::MapImportMergeNote)
    };
    ui.add(egui::Label::new(RichText::new(note).size(12.5).color(theme.muted)).wrap());
}

/// Read a picked file (at most the map file limit) and turn it into a map.
pub fn read_path(path: &Path, progress: &dyn Fn(Progress)) -> Result<Prepared, ImportError> {
    use std::io::Read;
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
    let file = std::fs::File::open(path).map_err(|e| ImportError::Invalid(e.to_string()))?;
    let mut bytes = Vec::new();
    file.take(wandur_core::map::format::MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ImportError::Invalid(e.to_string()))?;
    mudlet::read(extension.as_deref(), &bytes, progress)
}

/// Bring a read map into a tracker as one undoable step: this client's file replaces the map,
/// Mudlet's is merged. Returns how many items changed.
pub fn apply_to(tracker: &mut RoomMapTracker, prepared: &Prepared) -> Result<usize, String> {
    if prepared.replaces() {
        if tracker.replace_map(&prepared.map) {
            Ok(prepared.map.rooms.len() + prepared.map.links.len() + prepared.map.labels.len())
        } else {
            Err(t(S::MapEditRejected).to_string())
        }
    } else {
        tracker.import_map(&prepared.map).map_err(|e| e.0)
    }
}

/// Import into a world's saved map without a session: load it, apply, save (merged).
pub fn import_detached(store: &MapStore, world: MapWorld, prepared: &Prepared) -> Result<Detached, String> {
    let saved = store.load(&world).map_err(|e| e.to_string())?;
    let mut tracker = saved.map_or_else(RoomMapTracker::new, RoomMapTracker::from_snapshot);
    let changed = apply_to(&mut tracker, prepared)?;
    if changed > 0 {
        store.save(&world, &tracker.snapshot()).map_err(|e| e.to_string())?;
    }
    Ok(Detached {
        world,
        tracker,
        changed,
    })
}

/// The file picker: map files (JSON, XML) and Mudlet's binary map, so picking one can be
/// explained.
#[cfg(feature = "native-dialogs")]
pub fn pick_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(t(S::MapImportTitle))
        .add_filter(t(S::MapImportFileTypes), &["json", "xml", "dat"])
        .pick_file()
}

#[cfg(not(feature = "native-dialogs"))]
pub fn pick_file() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests;
