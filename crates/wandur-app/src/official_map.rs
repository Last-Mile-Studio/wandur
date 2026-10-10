//! A game's official map in a session ([`wandur_core::map::official`]): when the server sends
//! GMCP `Client.Map`, a strip in the session notice's style says "This game offers an official
//! map." with Download, Not now and Never.
//!
//! - Never is saved for the world (`official-maps/<world>/meta.json`) and the strip never shows
//!   for it again; Not now hides it for the rest of the session.
//! - When a file was imported before, the server is asked first (with the file's ETag, or its
//!   Last-Modified date), at most once a day per world unless the address changed; the strip
//!   shows only when the file changed.
//! - Download fetches and reads the file on a worker thread; the app then merges it into the
//!   session's map as one undoable step ([`wandur_core::map::official::merge`]) and the file is
//!   kept, on another worker thread, as the base of the next merge.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::{RichText, Ui};
use wandur_core::l10n::{S, t, tf};
use wandur_core::map::MapSnapshot;
use wandur_core::map::mudlet::{self, Prepared};
use wandur_core::map::official::download::{DownloadError, Downloaded};
use wandur_core::map::official::merge::MergeReport;
use wandur_core::map::official::offer::{self, Offer};
use wandur_core::map::official::store::{self, OfficialStore, Record, Validators};

use crate::theme::Theme;

/// Fetches a map file: the address and what is known of the file the client has (sent so the
/// server can answer 304). The real one is [`default_fetch`]; tests use their own.
pub type Fetch = Arc<dyn Fn(&str, &Validators) -> Result<Downloaded, DownloadError> + Send + Sync>;

/// Map downloads over https with the operating system's certificate verifier.
pub fn default_fetch() -> Fetch {
    let agent = wandur_core::map::official::download::agent();
    Arc::new(move |url: &str, known: &Validators| wandur_core::map::official::download::download(&agent, url, known))
}

type Wake = Arc<dyn Fn() + Send + Sync>;

/// A downloaded file, read and ready to merge.
pub struct Ready {
    pub prepared: Prepared,
    /// The file imported last time, read (the merge's base).
    pub base: Option<MapSnapshot>,
    pub bytes: Vec<u8>,
    /// The server's ETag and Last-Modified for it.
    pub validators: Validators,
}

/// Where the offer is.
pub enum Phase {
    /// Nothing to show (Not now, Never, unchanged, or not offered).
    Silent,
    /// Asking the server whether the imported file changed.
    Checking,
    /// The strip asks.
    Asking,
    Downloading,
    /// Downloaded and read: waiting for the app to merge it (once the session's map is loaded).
    Ready(Box<Ready>),
    /// A line to show in the strip (a failure, or that the map is up to date), with ×.
    Message(String),
}

enum Message {
    /// The check's answer: a changed file (and its ETag), or `None` to stay silent.
    Checked(Option<(Vec<u8>, Validators)>),
    /// The download: read and ready, `None` when the file had not changed, or why it failed.
    Read(Result<Option<Box<Ready>>, String>),
}

/// One session's offer.
pub struct OfficialMap {
    pub url: String,
    /// The world's folder name ([`store::world_key`]).
    pub key: String,
    store: OfficialStore,
    pub phase: Phase,
    /// Not now was chosen in this session.
    pub not_now: bool,
    /// A changed file the check already fetched, for Download to use.
    held: Option<(Vec<u8>, Validators)>,
    worker: Option<Receiver<Message>>,
    saving: Option<Receiver<Result<(), String>>>,
    fetch: Fetch,
    wake: Wake,
}

impl OfficialMap {
    /// The server named its map at `url`: decide whether to ask (checking with the server
    /// first when that file was imported before).
    pub fn new(url: &str, key: &str, store: OfficialStore, not_now: bool, fetch: Fetch, wake: Wake) -> Self {
        let mut map = Self {
            url: url.to_string(),
            key: key.to_string(),
            store,
            phase: Phase::Silent,
            not_now,
            held: None,
            worker: None,
            saving: None,
            fetch,
            wake,
        };
        let record = map.store.load(key);
        match offer::offer(&record, url, not_now, store::now_secs()) {
            Offer::Silent => {}
            Offer::Ask => map.phase = Phase::Asking,
            Offer::Check => map.check(record),
        }
        map
    }

    fn check(&mut self, record: Record) {
        let (send, receive) = channel();
        let (url, key, store, fetch, wake) = (
            self.url.clone(),
            self.key.clone(),
            self.store.clone(),
            Arc::clone(&self.fetch),
            Arc::clone(&self.wake),
        );
        let spawned = std::thread::Builder::new()
            .name("wandur-official-map".into())
            .spawn(move || {
                // Validators only for the very address imported: a new address is fetched whole.
                let changed = match fetch(&url, &record.validators(&url)) {
                    // A failed check stays silent (and is tried again next session): the
                    // person can still import by hand.
                    Err(_) => None,
                    Ok(Downloaded::NotModified) => {
                        let _ = store.save(&key, &checked(record, &url, false));
                        None
                    }
                    Ok(Downloaded::File {
                        bytes,
                        etag,
                        last_modified,
                    }) => {
                        let sha = store::sha256(&bytes);
                        if offer::after_check(&record, Some(&sha)) == Offer::Ask {
                            let _ = store.save(&key, &checked(record, &url, true));
                            Some((bytes, Validators { etag, last_modified }))
                        } else {
                            // The same file, maybe under a new address, ETag or date: remember
                            // them, so the next check can get a 304.
                            let record = Record {
                                url: Some(url.clone()),
                                etag,
                                last_modified,
                                ..record
                            };
                            let _ = store.save(&key, &checked(record, &url, false));
                            None
                        }
                    }
                };
                let _ = send.send(Message::Checked(changed));
                wake();
            });
        if spawned.is_ok() {
            self.worker = Some(receive);
            self.phase = Phase::Checking;
        }
    }

    /// Whether the strip shows.
    pub fn visible(&self) -> bool {
        matches!(self.phase, Phase::Asking | Phase::Downloading | Phase::Message(_))
    }

    /// Take what the workers sent.
    pub fn poll(&mut self) {
        if let Some(saving) = &self.saving {
            match saving.try_recv() {
                Ok(result) => {
                    self.saving = None;
                    if let Err(e) = result {
                        self.phase = Phase::Message(tf(S::OfficialMapSaveFailed, &[&e]));
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.saving = None,
            }
        }
        let Some(worker) = &self.worker else { return };
        let message = match worker.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.worker = None;
                self.phase = Phase::Silent;
                return;
            }
        };
        self.worker = None;
        match message {
            Message::Checked(None) => self.phase = Phase::Silent,
            Message::Checked(Some(file)) => {
                self.held = Some(file);
                self.phase = Phase::Asking;
            }
            Message::Read(Ok(Some(ready))) => self.phase = Phase::Ready(ready),
            Message::Read(Ok(None)) => self.phase = Phase::Message(t(S::OfficialMapUpToDate).into()),
            Message::Read(Err(e)) => self.phase = Phase::Message(tf(S::OfficialMapFailed, &[&e])),
        }
    }

    /// Whether a worker is still running.
    pub fn busy(&self) -> bool {
        self.worker.is_some() || self.saving.is_some()
    }

    /// Not now: hidden for the rest of the session.
    pub fn not_now(&mut self) {
        self.not_now = true;
        self.held = None;
        self.phase = Phase::Silent;
    }

    /// Never: saved for the world. Fails (the strip stays) when it cannot be saved.
    pub fn never(&mut self) -> Result<(), String> {
        let record = Record {
            never: true,
            ..self.store.load(&self.key)
        };
        self.store.save(&self.key, &record).map_err(|e| e.to_string())?;
        self.held = None;
        self.phase = Phase::Silent;
        Ok(())
    }

    /// The × on a message.
    pub fn dismiss(&mut self) {
        self.phase = Phase::Silent;
    }

    /// Download: fetch (unless the check already did), read, and read the base.
    pub fn download(&mut self) {
        let (send, receive) = channel();
        let (url, key, store, fetch, wake) = (
            self.url.clone(),
            self.key.clone(),
            self.store.clone(),
            Arc::clone(&self.fetch),
            Arc::clone(&self.wake),
        );
        let held = self.held.take();
        let spawned = std::thread::Builder::new()
            .name("wandur-official-map".into())
            .spawn(move || {
                let _ = send.send(Message::Read(fetch_and_read(&url, &key, &store, held, &fetch)));
                wake();
            });
        match spawned {
            Ok(_) => {
                self.worker = Some(receive);
                self.phase = Phase::Downloading;
            }
            Err(e) => self.phase = Phase::Message(tf(S::OfficialMapFailed, &[&e])),
        }
    }

    /// The read file, once ready, for the app to merge (taken once).
    pub fn take_ready(&mut self) -> Option<Box<Ready>> {
        match std::mem::replace(&mut self.phase, Phase::Silent) {
            Phase::Ready(ready) => Some(ready),
            other => {
                self.phase = other;
                None
            }
        }
    }

    /// The merge is done: keep the file as the next merge's base, on a worker thread.
    pub fn keep(&mut self, ready: Box<Ready>, report: MergeReport) {
        let (send, receive) = channel();
        let (url, key, store, wake) = (
            self.url.clone(),
            self.key.clone(),
            self.store.clone(),
            Arc::clone(&self.wake),
        );
        let spawned = std::thread::Builder::new()
            .name("wandur-official-map".into())
            .spawn(move || {
                let name = store::file_name(&ready.bytes);
                let record = Record {
                    url: Some(url.clone()),
                    etag: ready.validators.etag.clone(),
                    last_modified: ready.validators.last_modified.clone(),
                    sha256: Some(store::sha256(&ready.bytes)),
                    imported_at: Some(store::now_text()),
                    file: Some(name.to_string()),
                    report: Some(report),
                    never: store.load(&key).never,
                    ..Record::default()
                };
                let record = checked(record, &url, false);
                let _ = send.send(store.keep(&key, name, &ready.bytes, &record).map_err(|e| e.to_string()));
                wake();
            });
        if spawned.is_ok() {
            self.saving = Some(receive);
        }
    }
}

/// The download worker's part: the file (held from the check, or fetched now), read; the base
/// (the file imported last time), read. `None` when the server says the file has not changed,
/// or it is the very file imported.
fn fetch_and_read(
    url: &str,
    key: &str,
    store: &OfficialStore,
    held: Option<(Vec<u8>, Validators)>,
    fetch: &Fetch,
) -> Result<Option<Box<Ready>>, String> {
    let record = store.load(key);
    let (bytes, validators) = match held {
        Some(file) => file,
        None => match fetch(url, &record.validators(url)).map_err(|e| e.to_string())? {
            Downloaded::NotModified => return Ok(None),
            Downloaded::File {
                bytes,
                etag,
                last_modified,
            } => (bytes, Validators { etag, last_modified }),
        },
    };
    if record.sha256.as_deref() == Some(store::sha256(&bytes).as_str()) {
        return Ok(None);
    }
    let prepared = mudlet::read(None, &bytes, &|_| {}).map_err(|e| e.to_string())?;
    // The base is the file imported last, whatever address it came from.
    let base = store
        .base(key, &record)
        .and_then(|b| mudlet::read(None, &b, &|_| {}).ok())
        .map(|p| p.map);
    Ok(Some(Box::new(Ready {
        prepared,
        base,
        bytes,
        validators,
    })))
}

/// The record with a check of `url` noted now, and whether it found a changed file.
fn checked(record: Record, url: &str, found_change: bool) -> Record {
    Record {
        last_checked_at: Some(store::now_text()),
        last_checked_url: Some(url.to_string()),
        check_found_change: found_change,
        ..record
    }
}

/// What the strip asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripAction {
    Download,
    NotNow,
    Never,
    Dismiss,
}

/// The strip under the toolbar for the shown session, in the session notice's style.
pub fn strip(ui: &mut Ui, phase: &Phase, theme: &Theme) -> Option<StripAction> {
    let mut action = None;
    egui::Panel::top("official-map-strip")
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
                let text = match phase {
                    Phase::Downloading => t(S::OfficialMapDownloading),
                    Phase::Message(text) => text.as_str(),
                    _ => t(S::OfficialMapOffer),
                };
                if matches!(phase, Phase::Downloading) {
                    ui.spinner();
                }
                ui.label(RichText::new(text).size(13.0).color(theme.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    match phase {
                        Phase::Asking => {
                            if ui
                                .add(egui::Button::new(RichText::new(t(S::OfficialMapNever)).size(12.0)))
                                .clicked()
                            {
                                action = Some(StripAction::Never);
                            }
                            if ui
                                .add(egui::Button::new(RichText::new(t(S::OfficialMapNotNow)).size(12.0)))
                                .clicked()
                            {
                                action = Some(StripAction::NotNow);
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(t(S::OfficialMapDownload))
                                            .size(12.0)
                                            .strong()
                                            .color(theme.on_primary()),
                                    )
                                    .fill(theme.primary()),
                                )
                                .clicked()
                            {
                                action = Some(StripAction::Download);
                            }
                        }
                        Phase::Message(_) => {
                            let dismiss = ui
                                .add(egui::Button::new(RichText::new("×").size(14.0)).min_size(egui::vec2(30.0, 26.0)))
                                .on_hover_text(t(S::ClickToDismiss));
                            crate::widgets::name(&dismiss, egui::WidgetType::Button, t(S::ClickToDismiss));
                            if dismiss.clicked() {
                                action = Some(StripAction::Dismiss);
                            }
                        }
                        _ => {}
                    }
                });
            });
        });
    action
}

#[cfg(test)]
mod tests;
