//! The undo toast: after a delete (map rooms or exits, a saved world, a script, a macro), a
//! small note at the bottom of the window, "Deleted 3 rooms · Undo", for seven seconds. The time
//! stands still while the pointer is over it. Undo takes the delete back; one toast at a time,
//! a new one replaces the old (whose delete is then no longer undoable from a toast; the map's
//! own Undo still is).
//!
//! The toast holds what is needed to undo, never a secret: a deleted saved world keeps its
//! password reference, and the app forgets the saved password only once the toast is gone
//! without Undo ([`WandurApp`](crate::app::WandurApp)'s `finish_toast`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, Id, Rect, RichText, Stroke, vec2};
use wandur_core::db::scripts::LibraryEntry;
use wandur_core::l10n::{S, t};
use wandur_core::settings::SavedWorld;

use crate::session_tab::SessionId;
use crate::theme::Theme;

/// How long a toast shows (not counting time under the pointer).
pub const SHOWN_FOR: Duration = Duration::from_secs(7);
/// In the main window the toast sits this far above the bottom of the dock: clear of a
/// session's composer and footer, so typing and the footer's buttons stay reachable.
pub const MAIN_LIFT: f32 = 92.0;
/// Elsewhere (the world editor's body) it sits this far above the bottom of its area.
pub const BOTTOM_GAP: f32 = 14.0;

/// What Undo restores.
#[derive(Clone, Debug, PartialEq)]
pub enum Undo {
    /// The session's map edit with this undo number ([`RoomMapTracker::last_edit`]).
    ///
    /// [`RoomMapTracker::last_edit`]: wandur_core::map::RoomMapTracker::last_edit
    Map { session: SessionId, edit: u64 },
    /// A saved world, at its index, exactly as it was (its password reference included), and
    /// the sessions that were opened from it.
    World {
        index: usize,
        world: Box<SavedWorld>,
        sessions: Vec<SessionId>,
    },
    /// A script of the world editor's draft library, at its index in the library.
    Script { entry: Box<LibraryEntry>, index: usize },
    /// A macro of the world editor's draft library, at its index in the library.
    Macro { entry: Box<LibraryEntry>, index: usize },
    /// A map imported into a world with no session open: the app keeps the world's map (by
    /// `key`) with this undo step.
    DetachedMap { key: u64, edit: u64 },
}

impl Undo {
    /// Shown in the world editor's window (its draft library), not the main window.
    pub fn in_editor(&self) -> bool {
        matches!(self, Undo::Script { .. } | Undo::Macro { .. })
    }
}

/// One toast.
#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub undo: Undo,
    /// Shown so far (time under the pointer not counted).
    shown: Duration,
    last_tick: Option<Instant>,
    /// The pointer was over it last frame.
    pub hovered: bool,
    /// Newer toasts have larger numbers (the app keeps only the newest).
    pub serial: u64,
}

static SERIAL: AtomicU64 = AtomicU64::new(1);

impl Toast {
    pub fn new(text: String, undo: Undo) -> Self {
        Self {
            text,
            undo,
            shown: Duration::ZERO,
            last_tick: None,
            hovered: false,
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Count the time since the last tick, unless the pointer was over the toast. Returns true
    /// once it has been shown long enough.
    pub fn tick(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last_tick
            && !self.hovered
        {
            self.shown += now.saturating_duration_since(last);
        }
        self.last_tick = Some(now);
        self.shown >= SHOWN_FOR
    }

    /// Time left before it goes (while not hovered).
    pub fn remaining(&self) -> Duration {
        SHOWN_FOR.saturating_sub(self.shown)
    }
}

/// What the person did with a toast this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    None,
    Undo,
}

/// Draw the toast centred near the bottom of `over` (the main window's dock, or the world
/// editor's body), `lift` points above its bottom edge, over everything else. Its clock is advanced first. Returns what was done and
/// whether it has run out (then nothing is drawn).
pub fn show(
    ctx: &egui::Context,
    id: Id,
    over: Rect,
    lift: f32,
    toast: &mut Toast,
    theme: &Theme,
    now: Instant,
) -> (Output, bool) {
    if toast.tick(now) {
        return (Output::None, true);
    }
    let mut out = Output::None;
    let area = egui::Area::new(id.with(toast.serial))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(over.center().x, over.bottom() - lift))
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme.menu_fill())
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(CornerRadius::same(6))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 2],
                    blur: 10,
                    spread: 0,
                    color: Color32::from_black_alpha(if theme.light { 40 } else { 90 }),
                })
                .inner_margin(egui::Margin {
                    left: 14,
                    right: 6,
                    top: 5,
                    bottom: 5,
                })
                .show(ui, |ui| {
                    ui.set_max_width((over.width() - 40.0).max(160.0));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.add(
                            egui::Label::new(RichText::new(&toast.text).size(13.0).color(theme.text))
                                .truncate()
                                .selectable(false),
                        );
                        ui.label(RichText::new("·").size(13.0).color(theme.muted));
                        let undo = egui::Button::new(RichText::new(t(S::Undo)).size(13.0).strong().color(theme.accent))
                            .frame_when_inactive(false)
                            .min_size(vec2(0.0, 26.0));
                        if ui.add(undo).clicked() {
                            out = Output::Undo;
                        }
                    });
                });
        });
    toast.hovered = area.response.contains_pointer()
        || ctx
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| area.response.rect.contains(p));
    if !toast.hovered {
        ctx.request_repaint_after(toast.remaining().max(Duration::from_millis(16)));
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast() -> Toast {
        Toast::new("Deleted 3 rooms".into(), Undo::Map { session: 1, edit: 4 })
    }

    #[test]
    fn a_toast_goes_after_seven_seconds_not_counting_time_under_the_pointer() {
        let start = Instant::now();
        let mut t = toast();
        assert!(!t.tick(start));
        assert!(!t.tick(start + Duration::from_secs(3)));
        // Hovered for ten seconds: the clock stands still.
        t.hovered = true;
        assert!(!t.tick(start + Duration::from_secs(13)));
        assert_eq!(t.remaining(), Duration::from_secs(4));
        t.hovered = false;
        assert!(!t.tick(start + Duration::from_secs(16)));
        assert!(t.tick(start + Duration::from_secs(17)));
    }

    #[test]
    fn newer_toasts_have_larger_numbers_and_only_scripts_and_macros_show_in_the_editor() {
        let a = toast();
        let b = toast();
        assert!(b.serial > a.serial);
        assert!(!a.undo.in_editor());
        let entry = LibraryEntry::new_macro("m", wandur_core::macros::MacroDefinition::starter());
        assert!(
            Undo::Macro {
                entry: Box::new(entry),
                index: 0
            }
            .in_editor()
        );
    }
}
