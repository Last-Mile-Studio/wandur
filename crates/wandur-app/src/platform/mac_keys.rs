//! The keyboard's modifier keys as macOS sees them, whatever window has the focus.

use objc2_app_kit::{NSEvent, NSEventModifierFlags};

/// Whether Shift is held now. `+[NSEvent modifierFlags]` reads the keyboard's state, not an
/// event stream, so it answers before the app has a window or the focus (Shift held while
/// Wandur starts from the Dock or Finder).
pub fn shift_held() -> bool {
    NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Shift)
}
