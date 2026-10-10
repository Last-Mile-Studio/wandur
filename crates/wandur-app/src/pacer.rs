//! Repaint pacing for network output (the redraw cap).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Paces repaints caused by network output: at most `fps` output frames a second. A session's
/// reader wakes the UI when its inbox goes from empty to non-empty. If the last output frame was
/// at least a frame interval ago, the wake repaints at once; otherwise it does nothing, because the
/// frame that last applied output already asked (from the UI thread) for a repaint when the
/// interval ends. Asking for a delayed repaint from the network thread does not work: eframe
/// paints as soon as such a request arrives. Input events repaint at once as usual, so typing and
/// the caret are never delayed; output that arrives meanwhile waits in the inbox. With `fps` 0
/// every wake repaints, but still one frame per wake rather than egui's two.
pub struct Pacer {
    pub(crate) epoch: Instant,
    interval_ns: AtomicU64,
    last_drain_ns: AtomicU64,
}

impl Pacer {
    pub fn new(fps: u32) -> Self {
        let pacer = Self {
            epoch: Instant::now(),
            interval_ns: AtomicU64::new(0),
            last_drain_ns: AtomicU64::new(0),
        };
        pacer.set_fps(fps);
        pacer
    }

    /// Change the cap (0: none).
    pub fn set_fps(&self, fps: u32) {
        let ns = if fps == 0 { 0 } else { 1_000_000_000 / u64::from(fps) };
        self.interval_ns.store(ns, Ordering::Relaxed);
    }

    pub fn interval(&self) -> Duration {
        Duration::from_nanos(self.interval_ns.load(Ordering::Relaxed))
    }

    /// Delay before the next output repaint, given the time now.
    pub fn delay(&self, now: Duration) -> Duration {
        let last = self.last_drain_ns.load(Ordering::Relaxed);
        if last == 0 {
            return Duration::ZERO;
        }
        (Duration::from_nanos(last) + self.interval()).saturating_sub(now)
    }

    /// Called from network threads.
    pub fn wake(&self, ctx: &egui::Context) {
        if self.delay(self.epoch.elapsed()).is_zero() {
            // Not `request_repaint()`: egui answers every zero-delay request with two frames.
            // A non-zero delay below one frame time means one frame, as soon as possible.
            ctx.request_repaint_after(Duration::from_millis(1));
        }
    }

    /// Called by the UI thread after it applied output: records the time and asks for the next
    /// output frame when it is due (from the UI thread, which eframe honours). egui shortens every
    /// delay by its predicted frame time, so that is added back.
    pub fn drained(&self, ctx: &egui::Context) {
        self.last_drain_ns
            .store((self.epoch.elapsed().as_nanos() as u64).max(1), Ordering::Relaxed);
        let interval = self.interval();
        if !interval.is_zero() {
            let predicted = Duration::from_secs_f32(ctx.input(|i| i.predicted_dt).clamp(0.0, 0.05));
            ctx.request_repaint_after(interval + predicted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_spaces_output_frames() {
        let pacer = Pacer::new(30);
        let start = pacer.epoch.elapsed();
        assert!(pacer.delay(start).is_zero(), "first output repaints at once");
        let ctx = egui::Context::default();
        pacer.drained(&ctx);
        let now = pacer.epoch.elapsed();
        let d = pacer.delay(now);
        assert!(d > Duration::from_millis(25) && d <= Duration::from_millis(34), "{d:?}");
        assert!(pacer.delay(now + Duration::from_millis(40)).is_zero());
        let uncapped = Pacer::new(0);
        uncapped.drained(&ctx);
        assert!(uncapped.delay(uncapped.epoch.elapsed()).is_zero());
        pacer.set_fps(0);
        assert!(pacer.delay(pacer.epoch.elapsed()).is_zero());
    }
}
