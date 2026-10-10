//! macOS: moves the window's own close, minimise and zoom buttons to where
//! [`crate::traffic_lights::layout`] puts them, and back.
//!
//! AppKit keeps the buttons in an `NSTitlebarView` inside an `NSTitlebarContainerView` that
//! spans the top of the window. Moving a button below that container would leave it clipped and
//! deaf to clicks, so on a band taller than the native title bar the container grows to the band
//! first (the same approach Electron's `trafficLightPosition` and Tauri's traffic light inset
//! take). Only frames change: no style mask, title bar height setting or private API is used.
//!
//! The unsafe code is the two places this module turns an AppKit pointer into a reference: the
//! window handle's `NSView` and a view's `superview`. Everything runs on the main thread (eframe
//! calls `ui` there), checked with [`MainThreadMarker`].

use eframe::Frame as EframeFrame;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::traffic_lights::{Frame, NativeLights, Want, layout};

/// The views a move touches.
struct Views {
    window: Retained<NSWindow>,
    buttons: [Retained<NSView>; 3],
    /// The buttons' superview (`NSTitlebarView`).
    bar: Retained<NSView>,
    /// Its superview (`NSTitlebarContainerView`), which sits at the top of the window's frame
    /// view.
    container: Retained<NSView>,
    frame_view: Retained<NSView>,
}

/// Remembers AppKit's own layout from before the first move, to put it back for System.
#[derive(Debug, Default)]
pub struct TrafficLights {
    native: Option<NativeLights>,
    moved: bool,
}

impl TrafficLights {
    /// Applies `want` to the window behind `frame`. Does nothing without an AppKit window (a
    /// headless capture) or off the main thread.
    pub fn apply(&mut self, frame: &EframeFrame, want: Want) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(views) = views(frame, mtm) else {
            return;
        };
        // AppKit's own check as well as egui's: full screen and minimised windows are left alone.
        let mask = views.window.styleMask();
        if mask.contains(NSWindowStyleMask::FullScreen) || views.window.isMiniaturized() {
            return;
        }
        let current = read(&views);
        match want {
            Want::Leave => {}
            Want::Place(placement) => {
                // The first sight, before any move, is AppKit's own layout.
                let native = *self.native.get_or_insert(current);
                let target = layout(
                    &NativeLights {
                        buttons: current.buttons,
                        bar_height: native.bar_height,
                    },
                    &placement,
                );
                write(&views, &target.buttons, target.bar_height);
                self.moved = true;
            }
            Want::Native => {
                if let (true, Some(native)) = (self.moved, self.native) {
                    write(&views, &native.buttons, native.bar_height);
                    self.moved = false;
                }
            }
        }
    }
}

fn views(frame: &EframeFrame, _mtm: MainThreadMarker) -> Option<Views> {
    let handle = frame.window_handle().ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    // SAFETY: winit's AppKit handle points at the window's content `NSView`, which lives as
    // long as the window, and the window outlives this frame's `ui` call; we are on the main
    // thread (`_mtm`), where AppKit views may be used. `retain` keeps it alive while we use it.
    let view: Retained<NSView> = unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>())? };
    let window = view.window()?;
    let close = window.standardWindowButton(NSWindowButton::CloseButton)?;
    let mini = window.standardWindowButton(NSWindowButton::MiniaturizeButton)?;
    let zoom = window.standardWindowButton(NSWindowButton::ZoomButton)?;
    // SAFETY: `superview` returns a retained reference to a live view in the window's own
    // hierarchy (the window is retained above); main thread as above.
    let (bar, container, frame_view) = unsafe {
        let bar = close.superview()?;
        let container = bar.superview()?;
        let frame_view = container.superview()?;
        (bar, container, frame_view)
    };
    Some(Views {
        window,
        buttons: [
            Retained::into_super(Retained::into_super(close)),
            Retained::into_super(Retained::into_super(mini)),
            Retained::into_super(Retained::into_super(zoom)),
        ],
        bar,
        container,
        frame_view,
    })
}

/// The buttons in top-left coordinates of the title bar view, and the container's height.
fn read(v: &Views) -> NativeLights {
    let bounds = v.bar.bounds();
    let flipped = v.bar.isFlipped();
    let buttons = std::array::from_fn(|i| {
        let f = v.buttons[i].frame();
        let top = if flipped {
            f.origin.y
        } else {
            bounds.size.height - f.origin.y - f.size.height
        };
        Frame {
            x: f.origin.x as f32,
            y: top as f32,
            width: f.size.width as f32,
            height: f.size.height as f32,
        }
    });
    NativeLights {
        buttons,
        bar_height: v.container.frame().size.height as f32,
    }
}

/// Sizes the title bar container to `bar_height` at the top of the window and places the buttons
/// (top-left coordinates). Writes only what differs, so a settle step on a placed window does
/// not trigger AppKit layout.
fn write(v: &Views, buttons: &[Frame; 3], bar_height: f32) {
    let h = f64::from(bar_height);
    let outer = v.frame_view.bounds();
    let c = v.container.frame();
    let y = if v.frame_view.isFlipped() {
        0.0
    } else {
        outer.size.height - h
    };
    let want = NSRect::new(NSPoint::new(c.origin.x, y), NSSize::new(c.size.width, h));
    if c != want {
        v.container.setFrame(want);
    }
    // The title bar view normally follows its container; make sure it does.
    let b = v.bar.frame();
    if b.size.height != h || b.origin.y != 0.0 {
        v.bar
            .setFrame(NSRect::new(NSPoint::new(b.origin.x, 0.0), NSSize::new(b.size.width, h)));
    }
    // Run any layout the resize left pending now, so it cannot put the buttons back after they
    // are placed below.
    v.container.layoutSubtreeIfNeeded();
    let flipped = v.bar.isFlipped();
    for (view, f) in v.buttons.iter().zip(buttons) {
        let top = f64::from(f.y);
        let height = view.frame().size.height;
        let origin = NSPoint::new(f64::from(f.x), if flipped { top } else { h - top - height });
        if view.frame().origin != origin {
            view.setFrameOrigin(origin);
        }
    }
}
