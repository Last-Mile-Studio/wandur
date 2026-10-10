//! Update checks in the app (the C# `MainWindow.Updates`). Shortly after start (20 seconds), and
//! then at most once a day while the app runs, the client asks the directory for the newest
//! release ([`UpdateService`]); when it is newer than this build, a strip in the session notice's
//! style says so, with Download (the downloads page in the browser), Release notes and Skip this
//! version. It is hidden while the active session takes private input, its buttons give the
//! keyboard focus back to where it was, and nothing is ever downloaded or installed. Automatic
//! checks fail silently and never retry in a loop; Help > Check for Updates asks at once and says
//! the result. Checks run on their own thread, never on the UI thread.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use wandur_core::l10n::{S, t, tf};
use wandur_core::settings::Settings;
use wandur_core::updates::{UpdateCheckResult, UpdateCheckStatus, UpdateInfo, UpdateService};

use crate::theme::Theme;

/// How long after start the first automatic check waits, so it never competes with startup or a
/// first connection.
pub const STARTUP_DELAY: Duration = Duration::from_secs(20);
/// How often the app looks whether a check is due again.
pub const TICK: Duration = Duration::from_secs(60 * 60);

/// What a finished check asks of the app.
#[derive(Debug, Default, PartialEq)]
pub struct Finished {
    /// Save this in the settings (`last_update_check`).
    pub record: Option<wandur_core::updates::UpdateCheckRecord>,
    /// Show this answer (Check for Updates): heading and message.
    pub answer: Option<(String, String)>,
}

/// What the notice strip asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    Open(String),
    Skip(String),
    Dismiss,
}

pub struct Updates {
    service: UpdateService,
    /// The release the strip offers.
    pub offered: Option<UpdateInfo>,
    /// The × was clicked (this run only).
    pub dismissed: bool,
    /// A check running: whether its answer is for the menu command, and its result.
    pending: Option<(bool, Receiver<UpdateCheckResult>)>,
    /// When the next automatic look is due (none for a build that never checks).
    next: Option<Instant>,
    /// The widget that had the keyboard focus before the pointer went to the strip.
    focus: Option<egui::Id>,
    /// Give the focus back to this widget next frame (after a strip button's click).
    pub restore_focus: Option<egui::Id>,
}

impl Updates {
    /// The remembered result shows at once; the first automatic check waits `delay`.
    pub fn new(service: UpdateService, settings: &Settings, delay: Duration) -> Self {
        let next = service.checks_allowed().then(|| Instant::now() + delay);
        Self {
            offered: service.offer(settings),
            service,
            dismissed: false,
            pending: None,
            next,
            focus: None,
            restore_focus: None,
        }
    }

    /// Use another service (the directory address changed); what is offered stays.
    pub fn replace_service(&mut self, service: UpdateService) {
        self.service = service;
    }

    pub fn service(&self) -> &UpdateService {
        &self.service
    }

    pub fn checking(&self) -> bool {
        self.pending.is_some()
    }

    /// Look now at the next tick, whatever the schedule said (tests).
    pub fn look_now(&mut self) {
        if self.next.is_some() {
            self.next = Some(Instant::now());
        }
    }

    /// When the app should wake for the next automatic look.
    pub fn next_due(&self) -> Option<Instant> {
        self.next
    }

    /// Whether the strip shows: something offered, not dismissed, and the active session's
    /// input not private.
    pub fn visible(&self, private: bool) -> bool {
        self.offered.is_some() && !self.dismissed && !private
    }

    fn start(&mut self, settings: &Settings, manual: bool, wake: Arc<dyn Fn() + Send + Sync>) {
        if let Some((pending_manual, _)) = &mut self.pending {
            // Already asking: the answer goes to the menu command too.
            *pending_manual |= manual;
            return;
        }
        let (tx, rx) = channel();
        let service = self.service.clone();
        let settings = settings.clone();
        let spawned = std::thread::Builder::new()
            .name("wandur-update-check".into())
            .spawn(move || {
                let _ = tx.send(service.check(&settings));
                wake();
            });
        if spawned.is_ok() {
            self.pending = Some((manual, rx));
        }
    }

    /// The automatic schedule: when the time has come, look whether a check is due (a day since
    /// the last) and start one.
    pub fn tick(&mut self, now: Instant, settings: &Settings, wake: Arc<dyn Fn() + Send + Sync>) {
        let Some(next) = self.next else { return };
        if now < next {
            return;
        }
        self.next = Some(now + TICK);
        if !self.checking() && self.service.is_due(settings) {
            self.start(settings, false, wake);
        }
    }

    /// Check for Updates: ask now, whatever the schedule. A build from source answers at once.
    pub fn check_now(&mut self, settings: &Settings, wake: Arc<dyn Fn() + Send + Sync>) -> Option<(String, String)> {
        if !self.service.checks_allowed() {
            return Some((
                tf(S::UpdateChecksOffInSourceBuild, &[&self.service.running_version()]),
                String::new(),
            ));
        }
        self.start(settings, true, wake);
        None
    }

    /// Collect a finished check (waiting for it when `block`). `settings` are the settings as
    /// they are now; the record still has to be saved by the caller.
    pub fn poll(&mut self, settings: &Settings, block: bool) -> Option<Finished> {
        let (manual, rx) = self.pending.as_ref()?;
        let result = if block {
            rx.recv().ok()
        } else {
            match rx.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => None,
            }
        };
        let manual = *manual;
        self.pending = None;
        let result = result?;
        let mut after = settings.clone();
        if let Some(record) = &result.record {
            after.last_update_check = Some(record.clone());
        }
        let running = self.service.running_version().to_string();
        let answer = match result.status {
            UpdateCheckStatus::Available => {
                if manual {
                    self.offered = result.latest.clone();
                    self.dismissed = false;
                } else {
                    self.offered = self.service.offer(&after);
                }
                None
            }
            UpdateCheckStatus::UpToDate => Some((tf(S::UpdateUpToDate, &[&running]), String::new())),
            UpdateCheckStatus::Failed | UpdateCheckStatus::Disabled => Some((
                t(S::UpdateUnreachable).to_string(),
                t(S::UpdateUnreachableHint).to_string(),
            )),
        };
        Some(Finished {
            record: result.record,
            answer: answer.filter(|_| manual),
        })
    }

    /// Skip the offered version: hidden for good; a newer one shows again.
    pub fn skip(&mut self) -> Option<String> {
        self.offered.take().map(|o| o.version)
    }

    /// The strip under the toolbar. Returns what was clicked.
    pub fn strip(&mut self, ui: &mut Ui, theme: &Theme) -> Option<NoticeAction> {
        let offered = self.offered.clone()?;
        let mut action = None;
        // The buttons give the keyboard focus back (the command box keeps it), as the C#
        // notice buttons never take focus: remember who had it before a press lands here.
        if let Some(id) = ui.memory(|m| m.focused()) {
            self.focus = Some(id);
        }
        let focused = self.focus;
        egui::Panel::top("update-notice")
            .frame(
                egui::Frame::new()
                    .fill(theme.panel)
                    .stroke(egui::Stroke::new(1.0, theme.border))
                    .inner_margin(egui::Margin {
                        left: 18,
                        right: 18,
                        top: 7,
                        bottom: 7,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(26.0);
                    ui.label(
                        RichText::new(tf(S::UpdateAvailable, &[&offered.version]))
                            .size(12.0)
                            .color(theme.text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        // The C# notice buttons: 12 point text, 12 points of padding, 32 high.
                        ui.spacing_mut().button_padding = egui::vec2(12.0, 4.0);
                        ui.spacing_mut().interact_size.y = 32.0;
                        let dismiss = ui
                            .add(egui::Button::new(RichText::new("×").size(12.0)).min_size(egui::vec2(32.0, 32.0)))
                            .on_hover_text(t(S::ClickToDismiss));
                        crate::widgets::name(&dismiss, egui::WidgetType::Button, t(S::ClickToDismiss));
                        if dismiss.clicked() {
                            action = Some(NoticeAction::Dismiss);
                        }
                        if ui
                            .add(egui::Button::new(RichText::new(t(S::UpdateSkipVersion)).size(12.0)))
                            .clicked()
                        {
                            action = Some(NoticeAction::Skip(offered.version.clone()));
                        }
                        if let Some(notes) = &offered.notes
                            && ui
                                .add(egui::Button::new(RichText::new(t(S::UpdateReleaseNotes)).size(12.0)))
                                .clicked()
                        {
                            action = Some(NoticeAction::Open(notes.clone()));
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(t(S::UpdateDownload))
                                        .size(12.0)
                                        .strong()
                                        .color(theme.on_primary()),
                                )
                                .fill(theme.primary()),
                            )
                            .clicked()
                        {
                            action = Some(NoticeAction::Open(offered.page.clone()));
                        }
                    });
                });
            });
        if action.is_some() {
            self.restore_focus = focused;
            if let Some(id) = focused {
                ui.memory_mut(|m| m.request_focus(id));
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::UNIX_EPOCH;
    use wandur_core::updates::{DOWNLOADS_PAGE, UpdateSource};

    /// A fake answer, as the C# UpdateNoticeTests' FakeSource.
    #[derive(Default)]
    pub struct FakeSource {
        pub version: Mutex<String>,
        pub fail: AtomicBool,
        pub calls: AtomicUsize,
    }
    impl UpdateSource for FakeSource {
        fn latest(&self) -> Result<UpdateInfo, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err("offline".into());
            }
            let version = self.version.lock().unwrap().clone();
            Ok(UpdateInfo {
                notes: Some(format!(
                    "https://github.com/Last-Mile-Studio/wandur/releases/tag/v{version}"
                )),
                version,
                page: DOWNLOADS_PAGE.into(),
            })
        }
    }

    fn wake() -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(|| {})
    }

    #[test]
    fn the_schedule_waits_then_asks_once_and_a_dev_build_never_asks() {
        let source = Arc::new(FakeSource::default());
        *source.version.lock().unwrap() = "0.1.6".into();
        let clock: wandur_core::updates::Clock = Arc::new(|| UNIX_EPOCH + Duration::from_secs(1_791_028_800));
        let service = UpdateService::new(source.clone(), clock.clone(), "0.1.5");
        let mut settings = Settings::default();
        let mut updates = Updates::new(service, &settings, Duration::from_secs(20));
        let start = Instant::now();
        updates.tick(start, &settings, wake());
        assert!(!updates.checking(), "not before the startup delay");
        updates.tick(start + Duration::from_secs(21), &settings, wake());
        assert!(updates.checking());
        let finished = updates.poll(&settings, true).unwrap();
        assert_eq!(finished.answer, None, "automatic checks are silent");
        settings.last_update_check = finished.record;
        assert_eq!(updates.offered.as_ref().unwrap().version, "0.1.6");
        assert!(updates.visible(false));
        assert!(!updates.visible(true), "hidden during private input");
        // An hour later it is not due again (a day has not passed).
        updates.tick(start + Duration::from_secs(21 + 3600), &settings, wake());
        assert!(!updates.checking());
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);

        let dev = UpdateService::new(source.clone(), clock, "0.0.0-dev");
        let mut dev = Updates::new(dev, &settings, Duration::ZERO);
        assert_eq!(dev.next_due(), None);
        dev.tick(Instant::now() + Duration::from_secs(99), &settings, wake());
        assert!(!dev.checking());
        let (heading, _) = dev.check_now(&settings, wake()).unwrap();
        assert_eq!(heading, tf(S::UpdateChecksOffInSourceBuild, &[&"0.0.0-dev"]));
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    }
}
