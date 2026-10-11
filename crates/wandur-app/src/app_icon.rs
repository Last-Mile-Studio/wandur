//! The app's icon for its windows: the Dock and the app switcher on macOS, the title bar and the
//! taskbar elsewhere (without it eframe shows its own). `assets/icons/app-icon-512.png` is
//! `icon-1024.png` on a dark rounded square on Apple's grid (see that folder's README).

use std::sync::{Arc, OnceLock};

/// The icon, decoded once (about 1 MB of pixels) and shared by every window.
pub fn icon() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        let bytes: &[u8] = include_bytes!("../assets/icons/app-icon-512.png");
        Arc::new(eframe::icon_data::from_png_bytes(bytes).unwrap_or_default())
    })
    .clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_icon_decodes_to_a_square() {
        let icon = super::icon();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);
    }
}
