//! Where macOS's own traffic lights (close, minimise, zoom) sit on a drawn skin, and when to put
//! them there again (the C# `MacTrafficLightInset`).
//!
//! Fleet and Armored draw a title band taller than AppKit's title bar, so the native buttons
//! would sit near the band's top. The app moves them so their centre is the band's centre, at
//! the skin's own left inset; System and full screen keep AppKit's position. AppKit puts the
//! buttons back on its own after a resize, a change of key window, leaving full screen and
//! similar events, so a change in any of those starts a short settle: the layout is applied at
//! once and then a few more times over the next half second.
//!
//! This module is the pure part (the layout and the schedule), built and tested on every
//! platform. The AppKit calls live in [`crate::platform::mac_traffic_lights`].

use std::time::{Duration, Instant};

use crate::skin::{SkinId, WindowSkin};

/// Where the buttons go: the band they centre in and their left inset, in points from the
/// window's top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The drawn band's height; the native title bar area grows to it when it is shorter.
    pub band_height: f32,
    /// The buttons' vertical centre.
    pub center_y: f32,
    /// The close button's left edge.
    pub left: f32,
}

/// What to do with the buttons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Want {
    /// Move them onto the drawn band.
    Place(Placement),
    /// Put back AppKit's own position if they were moved (System).
    Native,
    /// Do not touch them: full screen (macOS shows them in the menu bar overlay) or minimised.
    Leave,
}

/// What the active skin wants for the buttons.
pub fn want(skin: &WindowSkin, full_screen: bool, minimized: bool) -> Want {
    if full_screen || minimized {
        return Want::Leave;
    }
    match skin.title_bar {
        Some(metrics) => Want::Place(Placement {
            band_height: metrics.band_height,
            center_y: metrics.traffic_lights_center(),
            left: metrics.traffic_lights_left,
        }),
        None => Want::Native,
    }
}

impl Placement {
    /// In native points under the egui zoom factor `zoom` (native points = egui points x zoom).
    pub fn scaled(self, zoom: f32) -> Placement {
        Placement {
            band_height: self.band_height * zoom,
            center_y: self.center_y * zoom,
            left: self.left * zoom,
        }
    }
}

/// A button's frame in points, from the top left of the title bar area (which starts at the
/// window's top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// What AppKit laid out: the three buttons (close, minimise, zoom) and the title bar area's
/// height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeLights {
    pub buttons: [Frame; 3],
    pub bar_height: f32,
}

/// Where the buttons go and how tall the title bar area must be to hold them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub buttons: [Frame; 3],
    pub bar_height: f32,
}

/// The buttons centred on `placement.center_y`, the close button at `placement.left` and the
/// others at AppKit's own spacing; their size is kept. The title bar area grows to the band (it
/// never shrinks below AppKit's height) so the buttons stay inside it and keep their clicks.
/// Edges land on whole points.
pub fn layout(native: &NativeLights, placement: &Placement) -> Layout {
    let first = native.buttons[0].x;
    let mut buttons = native.buttons;
    for b in &mut buttons {
        b.x = (placement.left + (b.x - first)).round();
        b.y = (placement.center_y - b.height / 2.0).round();
    }
    let lowest = buttons.iter().map(|b| b.y + b.height).fold(0.0, f32::max);
    Layout {
        buttons,
        bar_height: native.bar_height.max(placement.band_height).max(lowest),
    }
}

/// The width of the three buttons from the close button's left edge to the zoom button's right
/// edge, as macOS 26 lays them out (three 16 point buttons 23 apart; older releases are
/// narrower). The drawn title keeps clear of `left + LIGHTS_SPAN` plus a gap.
pub const LIGHTS_SPAN: f32 = 62.0;

/// Everything that, when it changes, may have made AppKit put the buttons back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trigger {
    pub skin: SkinId,
    /// The window's inner size in whole points.
    pub size: [i32; 2],
    pub focused: bool,
    pub full_screen: bool,
    pub minimized: bool,
    /// Changes with the colour scheme (a light or dark appearance can relayout the title bar).
    pub theme: u64,
    /// Points per native point (the UI zoom), as bits.
    pub zoom: u32,
}

/// How long one settle step waits, and how many steps follow the first apply.
pub const SETTLE_STEP: Duration = Duration::from_millis(100);
pub const SETTLE_STEPS: u8 = 6;

/// What a frame should do about the buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Poll {
    /// Apply the layout now.
    pub apply: bool,
    /// Ask for a frame after this long, for the next settle step.
    pub wake_after: Option<Duration>,
}

/// Applies the layout when a [`Trigger`] changes and a few times after (AppKit finishes some
/// transitions late); quiet otherwise, so a still window costs nothing.
#[derive(Debug, Default)]
pub struct Scheduler {
    last: Option<Trigger>,
    next: Option<Instant>,
    remaining: u8,
}

impl Scheduler {
    pub fn poll(&mut self, trigger: Trigger, now: Instant) -> Poll {
        if self.last != Some(trigger) {
            // A new change restarts the settle; repeated changes (a live resize) apply each
            // frame and keep pushing the tail out.
            self.last = Some(trigger);
            self.remaining = SETTLE_STEPS;
            self.next = Some(now + SETTLE_STEP);
            return Poll {
                apply: true,
                wake_after: Some(SETTLE_STEP),
            };
        }
        let Some(next) = self.next else {
            return Poll::default();
        };
        if now < next {
            return Poll {
                apply: false,
                wake_after: Some(next - now),
            };
        }
        self.remaining = self.remaining.saturating_sub(1);
        self.next = (self.remaining > 0).then(|| now + SETTLE_STEP);
        Poll {
            apply: true,
            wake_after: self.next.map(|_| SETTLE_STEP),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::TitleBarMetrics;

    /// macOS 26's layout: 16 point buttons at 8, 31 and 54, in a 32 point title bar.
    fn tahoe() -> NativeLights {
        let b = |x| Frame {
            x,
            y: 8.0,
            width: 16.0,
            height: 16.0,
        };
        NativeLights {
            buttons: [b(8.0), b(31.0), b(54.0)],
            bar_height: 32.0,
        }
    }

    fn center(f: &Frame) -> f32 {
        f.y + f.height / 2.0
    }

    fn placement(skin: &WindowSkin, full_screen: bool) -> Option<Placement> {
        match want(skin, full_screen, false) {
            Want::Place(p) => Some(p),
            _ => None,
        }
    }

    #[test]
    fn system_restores_and_full_screen_and_minimised_leave_them_alone() {
        assert_eq!(want(&WindowSkin::SYSTEM, false, false), Want::Native);
        for skin in [WindowSkin::FLEET, WindowSkin::ARMORED, WindowSkin::SYSTEM] {
            assert_eq!(want(&skin, true, false), Want::Leave);
            assert_eq!(want(&skin, false, true), Want::Leave);
        }
        assert!(placement(&WindowSkin::FLEET, false).is_some());
        let p = placement(&WindowSkin::ARMORED, false).unwrap().scaled(1.5);
        assert_eq!((p.band_height, p.center_y, p.left), (90.0, 34.5, 24.0));
    }

    #[test]
    fn the_drawn_skins_level_the_buttons_with_the_title_buttons() {
        // Fleet: the middle of its 38 point band. Armored: the upper plate's middle, at 23.
        for (skin, band, mid) in [(WindowSkin::FLEET, 38.0, 19.0), (WindowSkin::ARMORED, 60.0, 23.0)] {
            let p = placement(&skin, false).unwrap();
            assert_eq!(p.band_height, band);
            assert_eq!(p.center_y, mid);
            let l = layout(&tahoe(), &p);
            for b in &l.buttons {
                assert!((center(b) - mid).abs() <= 0.5, "{b:?} in {band}");
                assert_eq!((b.width, b.height), (16.0, 16.0));
            }
            assert_eq!(l.bar_height, band);
            assert_eq!(l.buttons[0].x, p.left);
            // AppKit's spacing is kept.
            assert_eq!(l.buttons[1].x - l.buttons[0].x, 23.0);
            assert_eq!(l.buttons[2].x - l.buttons[0].x, 46.0);
        }
        let fleet = layout(&tahoe(), &placement(&WindowSkin::FLEET, false).unwrap());
        assert_eq!(
            fleet.buttons[0],
            Frame {
                x: 12.0,
                y: 11.0,
                width: 16.0,
                height: 16.0
            }
        );
        let armored = layout(&tahoe(), &placement(&WindowSkin::ARMORED, false).unwrap());
        assert_eq!(
            armored.buttons[2],
            Frame {
                x: 62.0,
                y: 15.0,
                width: 16.0,
                height: 16.0
            }
        );
    }

    #[test]
    fn the_layout_follows_any_band_and_older_button_sizes() {
        // macOS 15 and earlier: 14 point buttons 20 apart at 7, in a 28 point title bar.
        let b = |x| Frame {
            x,
            y: 7.0,
            width: 14.0,
            height: 14.0,
        };
        let native = NativeLights {
            buttons: [b(7.0), b(27.0), b(47.0)],
            bar_height: 28.0,
        };
        for band in [24.0, 38.0, 45.0, 60.0, 80.0] {
            let p = Placement {
                band_height: band,
                center_y: band / 2.0,
                left: 14.0,
            };
            let l = layout(&native, &p);
            for b in &l.buttons {
                assert!((center(b) - band / 2.0).abs() <= 0.5);
                assert!(b.y + b.height <= l.bar_height);
            }
            // Never shorter than AppKit's own bar.
            assert_eq!(l.bar_height, band.max(28.0));
            assert_eq!([l.buttons[1].x, l.buttons[2].x], [34.0, 54.0]);
        }
    }

    #[test]
    fn the_reserved_space_clears_the_moved_buttons() {
        for m in [TitleBarMetrics::FLEET, TitleBarMetrics::ARMORED] {
            let l = layout(
                &tahoe(),
                &Placement {
                    band_height: m.band_height,
                    center_y: m.traffic_lights_center(),
                    left: m.traffic_lights_left,
                },
            );
            let right = l.buttons[2].x + l.buttons[2].width;
            assert!(crate::skin::mac_caption_left(Some(&m)) >= right + 12.0);
            assert_eq!(right, m.traffic_lights_left + LIGHTS_SPAN);
        }
    }

    fn trigger() -> Trigger {
        Trigger {
            skin: SkinId::Armored,
            size: [1100, 700],
            focused: true,
            full_screen: false,
            minimized: false,
            theme: 1,
            zoom: 1.0f32.to_bits(),
        }
    }

    /// Polls every 10 ms from `start` for `ms` and returns the offsets (ms) that applied.
    fn run(s: &mut Scheduler, start: Instant, from: u64, to: u64, t: Trigger) -> Vec<u64> {
        (from..to)
            .step_by(10)
            .filter(|ms| s.poll(t, start + Duration::from_millis(*ms)).apply)
            .collect()
    }

    #[test]
    fn the_first_frame_applies_then_settles_and_goes_quiet() {
        let mut s = Scheduler::default();
        let t0 = Instant::now();
        let first = s.poll(trigger(), t0);
        assert_eq!(
            first,
            Poll {
                apply: true,
                wake_after: Some(SETTLE_STEP)
            }
        );
        let applied = run(&mut s, t0, 10, 2000, trigger());
        assert_eq!(applied, vec![100, 200, 300, 400, 500, 600]);
        // Quiet afterwards: no apply, no wake.
        assert_eq!(s.poll(trigger(), t0 + Duration::from_secs(5)), Poll::default());
    }

    #[test]
    fn every_trigger_restarts_the_settle() {
        let changes: [fn(&mut Trigger); 6] = [
            |t| t.size = [1200, 700],
            |t| t.focused = false,
            |t| t.full_screen = true,
            |t| t.minimized = true,
            |t| t.theme = 2,
            |t| t.skin = SkinId::Fleet,
        ];
        for change in changes {
            let mut s = Scheduler::default();
            let t0 = Instant::now();
            s.poll(trigger(), t0);
            run(&mut s, t0, 10, 2000, trigger());
            let mut next = trigger();
            change(&mut next);
            let later = t0 + Duration::from_secs(3);
            assert!(s.poll(next, later).apply, "{next:?}");
            assert_eq!(run(&mut s, later, 10, 1000, next).len(), SETTLE_STEPS as usize);
            // And changing back (leaving full screen, focus returning) applies again.
            assert!(s.poll(trigger(), later + Duration::from_secs(2)).apply);
        }
    }

    #[test]
    fn a_live_resize_applies_each_frame_and_settles_after_the_last() {
        let mut s = Scheduler::default();
        let t0 = Instant::now();
        let mut t = trigger();
        for i in 0..20 {
            t.size[0] = 1100 + i;
            assert!(s.poll(t, t0 + Duration::from_millis(16 * i as u64)).apply);
        }
        let last = t0 + Duration::from_millis(16 * 19);
        let wake = s.poll(t, last + Duration::from_millis(30));
        assert_eq!(
            wake,
            Poll {
                apply: false,
                wake_after: Some(Duration::from_millis(70))
            }
        );
        assert_eq!(run(&mut s, last, 10, 1500, t).len(), SETTLE_STEPS as usize);
    }
}
