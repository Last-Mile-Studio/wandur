//! Opt-in window capture for checking the UI without watching the screen.
//!
//! `WANDUR_SCREENSHOT=/path/shot.png` (or `.bmp`) saves the rendered window once
//! `WANDUR_SCREENSHOT_AFTER` seconds (default 3) have passed; `WANDUR_SCREENSHOT_EXIT=1` then
//! closes the app. Nothing runs when the variable is unset.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{ColorImage, Event, ViewportCommand};

pub struct Screenshot {
    path: PathBuf,
    due: Instant,
    requested: bool,
    exit: bool,
}

impl Screenshot {
    pub fn from_env() -> Option<Screenshot> {
        let path = std::env::var("WANDUR_SCREENSHOT").ok().filter(|p| !p.is_empty())?;
        let after = std::env::var("WANDUR_SCREENSHOT_AFTER")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(3.0);
        Some(Screenshot {
            path: PathBuf::from(path),
            due: Instant::now() + Duration::from_secs_f64(after),
            requested: false,
            exit: std::env::var("WANDUR_SCREENSHOT_EXIT").is_ok(),
        })
    }

    /// Call once per frame. Returns true when the app should close.
    pub fn on_frame(&mut self, ctx: &egui::Context) -> bool {
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let saved = if self.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
                write_png(&self.path, &image)
            } else {
                write_bmp(&self.path, &image)
            };
            if let Err(e) = saved {
                eprintln!("wandur: could not save the screenshot: {e}");
            }
            return self.exit;
        }
        if !self.requested {
            let now = Instant::now();
            if now >= self.due {
                self.requested = true;
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
            }
            ctx.request_repaint_after(self.due.saturating_duration_since(now));
        }
        false
    }
}

fn write_png(path: &PathBuf, image: &ColorImage) -> std::io::Result<()> {
    let [w, h] = image.size;
    let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    image::save_buffer(path, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8).map_err(std::io::Error::other)
}

fn write_bmp(path: &PathBuf, image: &ColorImage) -> std::io::Result<()> {
    let [w, h] = image.size;
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        let start = out.len();
        for p in &image.pixels[y * w..(y + 1) * w] {
            out.extend_from_slice(&[p.b(), p.g(), p.r()]);
        }
        out.resize(start + row, 0);
    }
    std::fs::write(path, out)
}
