//! File > Import from Wandur (C#)... and `wandur --import-csharp`: bring the C# client's data
//! over with [`wandur_core::import::csharp`].
//!
//! The dialog picks the C# data folder (its default is where the C# client keeps it), shows what
//! it holds (counts only), imports on a worker thread with progress, and ends with the summary:
//! counts per kind and why anything was skipped. Saved passwords and agent API keys are copied
//! only when the box asking for them is ticked (the system may ask the person to allow each).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::{Align, Layout, RichText};
use wandur_core::db::Database;
use wandur_core::import::csharp::{
    self, CsharpSource, ImportOptions, Kind, Progress, Report, default_csharp_dir, secrets,
};
use wandur_core::l10n::{S, t, tf};
use wandur_core::login::PasswordVault;
use wandur_core::settings::Settings;

use crate::dialogs::{modal, primary_button, secondary_button};
use crate::theme::Theme;

/// What the person asked for this frame.
#[derive(Debug, PartialEq)]
pub enum ImportAction {
    Open,
    Closed,
    /// Choose the folder with the system's folder picker.
    ChooseFolder,
    /// Start the import (the app hands over its database, settings and vault).
    Start,
    /// The import committed: the app takes these settings (worlds and preferences).
    Apply(Box<Settings>),
}

enum Message {
    Progress(Progress),
    Done(Box<Result<(Report, Settings), String>>),
}

enum State {
    /// The folder can be edited; `found` is what a look found in it.
    Choosing {
        found: Option<Result<Report, String>>,
    },
    Looking(Receiver<Result<Report, String>>),
    Importing {
        messages: Receiver<Message>,
        progress: Option<Progress>,
    },
    /// The summary, or why the import stopped.
    Finished(Result<Report, String>),
}

/// The dialog.
pub struct CsharpImportDialog {
    pub folder: String,
    pub include_secrets: bool,
    state: State,
    /// Settings waiting for the app (handed over once).
    pending: Option<Settings>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl CsharpImportDialog {
    pub fn new(folder: Option<PathBuf>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let folder = folder
            .or_else(default_csharp_dir)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let mut dialog = Self {
            folder,
            include_secrets: true,
            state: State::Choosing { found: None },
            pending: None,
            wake,
        };
        dialog.look();
        dialog
    }

    /// Count what the folder holds, on a worker thread.
    pub fn look(&mut self) {
        let folder = PathBuf::from(self.folder.trim());
        let (send, receive) = channel();
        let wake = Arc::clone(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("wandur-csharp-look".into())
            .spawn(move || {
                let found = CsharpSource::open(&folder)
                    .and_then(|source| source.found())
                    .map_err(|e| e.to_string());
                let _ = send.send(found);
                wake();
            });
        self.state = match spawned {
            Ok(_) => State::Looking(receive),
            Err(e) => State::Choosing {
                found: Some(Err(e.to_string())),
            },
        };
    }

    /// Import into `db` and a copy of `settings` on a worker thread; secrets go from `from` (the
    /// C# entries) to `to` when the box is ticked.
    pub fn start(
        &mut self,
        db: Database,
        settings: Settings,
        from: Arc<dyn PasswordVault>,
        to: Arc<dyn PasswordVault>,
    ) {
        let folder = PathBuf::from(self.folder.trim());
        let include_secrets = self.include_secrets;
        let (send, receive) = channel();
        let wake = Arc::clone(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("wandur-csharp-import".into())
            .spawn(move || {
                let progress = |p: Progress| {
                    let _ = send.send(Message::Progress(p));
                    wake();
                };
                let result = (|| {
                    let source = CsharpSource::open(&folder).map_err(|e| e.to_string())?;
                    let outcome = csharp::import(&source, &db, &settings, false, &progress, &mut |_| Ok(()))
                        .map_err(|e| e.to_string())?;
                    let mut report = outcome.report;
                    if include_secrets {
                        secrets::copy(
                            &outcome.secrets,
                            from.as_ref(),
                            to.as_ref(),
                            &mut report,
                            &|done, total| {
                                progress(Progress {
                                    kind: Kind::Passwords,
                                    done,
                                    total,
                                })
                            },
                        );
                    } else {
                        secrets::not_requested(&outcome.secrets, &mut report);
                    }
                    Ok((report, outcome.settings))
                })();
                let _ = send.send(Message::Done(Box::new(result)));
                wake();
            });
        self.state = match spawned {
            Ok(_) => State::Importing {
                messages: receive,
                progress: None,
            },
            Err(e) => State::Finished(Err(e.to_string())),
        };
    }

    /// Take what the workers sent.
    pub fn poll(&mut self) {
        match &mut self.state {
            State::Looking(receive) => {
                if let Ok(found) = receive.try_recv() {
                    self.state = State::Choosing { found: Some(found) };
                }
            }
            State::Importing { messages, progress } => {
                let mut finished = None;
                while let Ok(message) = messages.try_recv() {
                    match message {
                        Message::Progress(p) => *progress = Some(p),
                        Message::Done(result) => finished = Some(*result),
                    }
                }
                if let Some(result) = finished {
                    self.state = State::Finished(result.map(|(report, settings)| {
                        self.pending = Some(settings);
                        report
                    }));
                }
            }
            _ => {}
        }
    }

    /// Whether a worker is still running.
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Looking(_) | State::Importing { .. })
    }

    /// The summary once the import finished.
    pub fn summary(&self) -> Option<&Result<Report, String>> {
        match &self.state {
            State::Finished(result) => Some(result),
            _ => None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme) -> ImportAction {
        self.poll();
        if let Some(settings) = self.pending.take() {
            return ImportAction::Apply(Box::new(settings));
        }
        let busy = self.busy();
        let mut action = ImportAction::Open;
        let response = modal(ctx, "csharp-import", 540.0, theme, |ui| {
            ui.label(RichText::new(t(S::CsImportTitle)).size(24.0).color(theme.text));
            ui.add_space(10.0);
            if let State::Finished(result) = &self.state {
                match result {
                    Ok(report) => {
                        ui.label(RichText::new(t(S::CsImportDone)).size(15.0).strong().color(theme.text));
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical()
                            .id_salt("csharp-import-summary")
                            .max_height(340.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(report.to_text()).size(13.0).color(theme.text))
                                        .wrap()
                                        .selectable(true),
                                );
                            });
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(RichText::new(t(S::CsImportSessionsNote)).size(12.0).color(theme.muted))
                                .wrap(),
                        );
                    }
                    Err(e) => {
                        ui.add(
                            egui::Label::new(RichText::new(tf(S::CsImportFailed, &[e])).size(13.0).color(theme.text))
                                .wrap(),
                        );
                    }
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_button(ui, t(S::Done), theme).clicked() {
                        action = ImportAction::Closed;
                    }
                });
                return;
            }
            ui.add(egui::Label::new(RichText::new(t(S::CsImportIntro)).size(13.0).color(theme.muted)).wrap());
            ui.add_space(12.0);
            ui.label(RichText::new(t(S::CsImportFolder)).size(13.0).color(theme.text));
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.folder)
                            .desired_width(330.0)
                            .hint_text(t(S::CsImportFolder)),
                    );
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.look();
                    }
                    if secondary_button(ui, t(S::CsImportChoose)).clicked() {
                        action = ImportAction::ChooseFolder;
                    }
                    if secondary_button(ui, t(S::CsImportLook)).clicked() {
                        self.look();
                    }
                });
            });
            ui.add_space(10.0);
            match &self.state {
                State::Looking(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(t(S::CsImportReading)).size(13.0).color(theme.muted));
                    });
                }
                State::Choosing {
                    found: Some(Ok(report)),
                } => {
                    ui.label(RichText::new(t(S::CsImportFound)).size(13.0).color(theme.text));
                    for kind in Kind::ALL {
                        let n = report.tally(kind).found;
                        if n > 0 {
                            ui.label(
                                RichText::new(format!("    {}", tf(S::CsImportFoundLine, &[&kind.label(), &n])))
                                    .size(13.0)
                                    .color(theme.muted),
                            );
                        }
                    }
                }
                State::Choosing { found: Some(Err(e)) } => {
                    ui.add(egui::Label::new(RichText::new(e.as_str()).size(13.0).color(theme.text)).wrap());
                }
                State::Importing { progress, .. } => {
                    let label = progress.map_or_else(
                        || t(S::CsImportReading).to_string(),
                        |p| tf(S::CsImportWorking, &[&p.kind.label()]),
                    );
                    let fraction = progress.map_or(0.0, |p| {
                        if p.total == 0 {
                            1.0
                        } else {
                            p.done as f32 / p.total as f32
                        }
                    });
                    ui.label(RichText::new(label).size(13.0).color(theme.text));
                    ui.add(egui::ProgressBar::new(fraction).desired_width(480.0));
                }
                _ => {}
            }
            ui.add_space(10.0);
            ui.add_enabled_ui(!busy, |ui| {
                ui.checkbox(
                    &mut self.include_secrets,
                    RichText::new(t(S::CsImportPasswords)).size(13.0),
                );
            });
            ui.add_space(12.0);
            let ready = matches!(self.state, State::Choosing { found: Some(Ok(_)) });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let import = ui
                    .add_enabled_ui(ready, |ui| primary_button(ui, t(S::CsImportStart), theme))
                    .inner;
                if import.clicked() {
                    action = ImportAction::Start;
                }
                let cancel = ui
                    .add_enabled_ui(!matches!(self.state, State::Importing { .. }), |ui| {
                        secondary_button(ui, t(S::Cancel2))
                    })
                    .inner;
                if cancel.clicked() {
                    action = ImportAction::Closed;
                }
            });
        });
        // Escape or a click outside closes it, except while the import runs (it cannot be
        // stopped half way: it is one transaction).
        if response.should_close() && !matches!(self.state, State::Importing { .. }) {
            return ImportAction::Closed;
        }
        action
    }
}

/// `wandur --import-csharp`: what to import and where.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CliImport {
    /// The C# data directory or its `wandur.db`.
    pub source: PathBuf,
    /// This client's data directory; the platform default when `None`.
    pub data_dir: Option<PathBuf>,
    pub dry_run: bool,
    pub include_passwords: bool,
}

/// Why the command line import did not run.
#[derive(Debug)]
pub enum CliError {
    /// No `--data-dir` and no platform data directory.
    NoDataDir,
    Import(csharp::ImportError),
}

/// Run the import from the command line. Returns what goes to standard output: counts per kind
/// and skip reasons only, never a name, an address, text or a secret.
pub fn run_cli(cli: &CliImport, from: &dyn PasswordVault, to: &dyn PasswordVault) -> Result<String, CliError> {
    let dir = cli
        .data_dir
        .clone()
        .or_else(wandur_core::settings::default_data_dir)
        .ok_or(CliError::NoDataDir)?;
    let source = CsharpSource::open(&cli.source).map_err(CliError::Import)?;
    let options = ImportOptions {
        dry_run: cli.dry_run,
        include_secrets: cli.include_passwords,
    };
    csharp::import_into_dir(&source, &dir, options, from, to, &|_| {})
        .map(|report| report.to_text())
        .map_err(CliError::Import)
}

/// The folder picker for the dialog.
#[cfg(feature = "native-dialogs")]
pub fn pick_folder(start: &Path) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(t(S::CsImportTitle));
    if start.is_dir() {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder()
}

#[cfg(not(feature = "native-dialogs"))]
pub fn pick_folder(_: &Path) -> Option<PathBuf> {
    None
}
