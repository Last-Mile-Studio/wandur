//! Help > About Wandur: a window of its own (moved, resized and closed as the settings are) with
//! three tabs on the left, in the settings' section list style:
//!
//! - About: the logo, name, version, what Wandur is, the studio, the licence, the bundled fonts'
//!   licences and the main open source libraries.
//! - System information: what helps a bug report (version, commit, build, operating system,
//!   renderer, display scale, data folder, features, open sessions, memory), with Copy. The
//!   home folder shows as `~`, and nothing from the settings or the sessions is named beyond a
//!   count, so a pasted report carries no world, host or account name.
//! - Links: the site's pages and the source code, opened in the browser through the app's
//!   launcher.
//!
//! Keyboard: Tab reaches the tabs, Up and Down move between them (the selection follows),
//! Enter or Space chooses one, Escape closes the window.

use std::path::{Path, PathBuf};

use egui::accesskit::Role;
use egui::{Align, Id, Layout, RichText, Sense, Ui};
use wandur_core::l10n::{self, Language, S, t, text_in};

use crate::dialogs;
use crate::theme::Theme;
use crate::widgets::{Icon, paint_icon};

/// This build's version (the workspace's).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The git commit it was built from, short (`unknown` without git; see `build.rs`).
pub const COMMIT: &str = env!("WANDUR_BUILD_COMMIT");
/// The Cargo profile (`debug` or `release`).
pub const PROFILE: &str = env!("WANDUR_BUILD_PROFILE");
/// The target triple.
pub const TARGET: &str = env!("WANDUR_BUILD_TARGET");

/// Who makes Wandur (a name, not translated).
pub const STUDIO: &str = "Last Mile Studio";
/// The year in the licence's copyright line (`LICENSE`).
const COPYRIGHT_YEAR: u32 = 2026;
/// The repository and its issue tracker.
pub const SOURCE_URL: &str = "https://github.com/Last-Mile-Studio/wandur";
pub const ISSUES_URL: &str = "https://github.com/Last-Mile-Studio/wandur/issues";
/// The bundled fonts and their licences (`assets/fonts`).
const FONTS: [(&str, &str); 2] = [
    ("JetBrains Mono", "SIL Open Font License 1.1"),
    ("Ubuntu Medium", "Ubuntu Font Licence 1.0"),
];
/// The main open source libraries the client is built on (names, not translated).
const LIBRARIES: &str = "egui, eframe, egui_dock, alacritty_terminal, QuickJS (rquickjs), Lua (mlua), \
SQLite (rusqlite), rustls, ureq, quick-xml, Biome, image";
#[cfg(target_os = "macos")]
const MACOS: &str = "macOS";
#[cfg(windows)]
const WINDOWS: &str = "Windows";

/// The three tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AboutTab {
    About,
    System,
    Links,
}

impl AboutTab {
    pub const ALL: [AboutTab; 3] = [AboutTab::About, AboutTab::System, AboutTab::Links];

    /// A tab by the name scenes use (`about`, `system`, `links`).
    pub fn from_name(name: &str) -> Option<AboutTab> {
        Some(match name {
            "about" => AboutTab::About,
            "system" => AboutTab::System,
            "links" => AboutTab::Links,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        t(match self {
            AboutTab::About => S::AboutTabAbout,
            AboutTab::System => S::AboutTabSystem,
            AboutTab::Links => S::AboutTabLinks,
        })
    }

    /// The tab's widget id (stable, so keyboard focus can follow it).
    pub(crate) fn id(self) -> Id {
        Id::new(("about-tab", self))
    }
}

/// One entry of the Links tab: its title and address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub title: S,
    pub url: String,
}

/// The Links tab's entries; the site's pages under `site` (the wandur.net base the Help menu
/// uses, so a test or a local site never opens the public one).
pub fn links(site: &str) -> Vec<Link> {
    let link = |title, url: String| Link { title, url };
    vec![
        link(S::AboutLinkWebsite, site.to_string()),
        link(S::AboutLinkFindAMud, wandur_core::site::page(site, "worlds")),
        link(S::AboutLinkOtherClients, wandur_core::site::other_clients(site)),
        link(S::AboutLinkSource, SOURCE_URL.to_string()),
        link(S::AboutLinkIssue, ISSUES_URL.to_string()),
    ]
}

/// What the System information tab shows and Copy puts on the clipboard. Only these facts,
/// never anything from the settings or a session beyond the count.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemInfo {
    pub version: String,
    pub commit: String,
    /// The profile and target: `debug, aarch64-apple-darwin`.
    pub build: String,
    pub os: String,
    pub arch: String,
    /// The renderer and graphics adapter, when the window reported them.
    pub renderer: Option<String>,
    /// Pixels per point.
    pub scale: f32,
    /// The data folder, the home folder shown as `~`.
    pub data_dir: Option<String>,
    pub features: Vec<(&'static str, bool)>,
    pub sessions: usize,
    /// Resident memory in bytes, where the platform tells cheaply.
    pub memory: Option<u64>,
}

impl SystemInfo {
    /// This process now.
    pub fn gather(renderer: Option<&str>, scale: f32, data_dir: Option<&Path>, sessions: usize) -> SystemInfo {
        let home = home_dir();
        SystemInfo {
            version: VERSION.to_string(),
            commit: COMMIT.to_string(),
            build: [PROFILE, TARGET].join(", "),
            os: os_description().to_string(),
            arch: std::env::consts::ARCH.to_string(),
            renderer: renderer.map(str::to_string),
            scale,
            data_dir: data_dir.map(|d| mask_home(d, home.as_deref())),
            features: wandur_core::FEATURES.to_vec(),
            sessions,
            memory: Some(crate::sysstat::resident_bytes()).filter(|&b| b > 0),
        }
    }

    /// The rows in `language`: a label and its value.
    pub fn rows(&self, language: Language) -> Vec<(&'static str, String)> {
        let text = |key| text_in(language, key);
        let missing = || text(S::AboutNotReported).to_string();
        let features: Vec<String> = self
            .features
            .iter()
            .map(|(name, on)| format!("{}{name}", if *on { '+' } else { '-' }))
            .collect();
        let memory = match self.memory {
            Some(bytes) => {
                let megabytes = (bytes as f64 / (1024.0 * 1024.0)).round() as u64;
                l10n::format(text(S::AboutMemoryMb), &[&megabytes])
            }
            None => missing(),
        };
        vec![
            (text(S::AboutSysVersion), self.version.clone()),
            (text(S::AboutSysCommit), self.commit.clone()),
            (text(S::AboutSysBuild), self.build.clone()),
            (text(S::AboutSysOs), self.os.clone()),
            (text(S::AboutSysArch), self.arch.clone()),
            (text(S::AboutSysRenderer), self.renderer.clone().unwrap_or_else(missing)),
            (text(S::AboutSysScale), format!("{}%", (self.scale * 100.0).round())),
            (text(S::AboutSysDataDir), self.data_dir.clone().unwrap_or_else(missing)),
            (text(S::AboutSysFeatures), features.join(" ")),
            (text(S::AboutSysSessions), self.sessions.to_string()),
            (text(S::AboutSysMemory), memory),
        ]
    }

    /// The plain text Copy puts on the clipboard: in English whatever the UI language, since
    /// it goes into an issue report.
    pub fn report(&self) -> String {
        let mut out = l10n::format(text_in(Language::En, S::AboutReportTitle), &[&dialogs::DISPLAY_NAME]);
        out.push('\n');
        for (label, value) in self.rows(Language::En) {
            out.push_str(&format!("{label}: {value}\n"));
        }
        out
    }
}

/// The person's home folder, from the environment.
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// `path` with `home` shown as `~`, so a copied report does not carry the person's name.
pub fn mask_home(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home.filter(|h| h.parent().is_some()) else {
        return path.display().to_string();
    };
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => Path::new("~").join(rest).display().to_string(),
        // Not under home: still never show the home folder's path should it appear inside
        // (as a whole folder, not the start of a longer name).
        Err(_) => {
            let text = path.display().to_string();
            let home = home.display().to_string();
            let mut out = String::new();
            let mut rest = text.as_str();
            while let Some(at) = rest.find(&home) {
                let after = &rest[at + home.len()..];
                let whole = after.is_empty() || after.starts_with(std::path::is_separator);
                out.push_str(&rest[..at]);
                out.push_str(if whole { "~" } else { &home });
                rest = after;
            }
            out.push_str(rest);
            out
        }
    }
}

/// The operating system's name and version, read once.
pub fn os_description() -> &'static str {
    static OS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OS.get_or_init(read_os)
}

#[cfg(target_os = "macos")]
fn read_os() -> String {
    let mut buf = [0u8; 64];
    let mut len = buf.len();
    // SAFETY: sysctlbyname writes at most `len` bytes into `buf` and sets `len` to the count.
    let ok = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) == 0
    };
    let version = if ok {
        String::from_utf8_lossy(&buf[..len.min(buf.len())])
            .trim_end_matches('\0')
            .to_string()
    } else {
        String::new()
    };
    [MACOS, version.as_str()].join(" ").trim().to_string()
}

#[cfg(target_os = "linux")]
fn read_os() -> String {
    // The distribution's name (`/etc/os-release`) and the kernel's release (uname).
    let pretty = std::fs::read_to_string("/etc/os-release").ok().and_then(|text| {
        text.lines()
            .find_map(|l| l.strip_prefix("PRETTY_NAME="))
            .map(|v| v.trim().trim_matches('"').to_string())
    });
    // SAFETY: uname fills the zeroed struct; its fields are NUL-terminated.
    let kernel = unsafe {
        let mut name: libc::utsname = std::mem::zeroed();
        (libc::uname(&mut name) == 0).then(|| {
            std::ffi::CStr::from_ptr(name.release.as_ptr())
                .to_string_lossy()
                .into_owned()
        })
    };
    let linux = kernel.map(|k| [std::env::consts::OS, k.as_str()].join(" "));
    match (pretty, linux) {
        (Some(p), Some(l)) => [p, l].join(", "),
        (Some(p), None) => p,
        (None, Some(l)) => l,
        (None, None) => std::env::consts::OS.to_string(),
    }
}

#[cfg(windows)]
fn read_os() -> String {
    WINDOWS.to_string()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn read_os() -> String {
    std::env::consts::OS.to_string()
}

/// What the window asks of the app this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AboutResult {
    Open,
    Closed,
    /// A link was chosen: open it in the browser.
    OpenLink(String),
}

/// The About window's state while it is open.
#[derive(Clone, Debug, PartialEq)]
pub struct AboutWindow {
    pub tab: AboutTab,
    /// The tab that had keyboard focus last frame: focus moving from one tab to another by
    /// the arrow keys chooses the tab it lands on.
    focused: Option<AboutTab>,
    /// When Copy was last pressed (input time), for the Copied note.
    copied_at: Option<f64>,
}

impl Default for AboutWindow {
    fn default() -> Self {
        Self::new(AboutTab::About)
    }
}

/// How long the Copied note stays.
const COPIED_SECONDS: f64 = 2.0;

impl AboutWindow {
    pub fn new(tab: AboutTab) -> Self {
        Self {
            tab,
            focused: None,
            copied_at: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, info: &SystemInfo, links: &[Link]) -> AboutResult {
        let mut result = AboutResult::Open;
        let spec = crate::dialog_window::Spec {
            id: "about-window",
            title: t(S::AboutWandur),
            default_size: egui::vec2(700.0, 560.0),
            min_size: egui::vec2(560.0, 440.0),
        };
        let close = crate::dialog_window::show(ctx, spec, theme, |ui| {
            let nav_width = 180.0;
            let footer = 62.0;
            let full = ui.max_rect();
            let nav = egui::Rect::from_min_size(full.min, egui::vec2(nav_width, full.height() - footer));
            let body = egui::Rect::from_min_max(
                egui::pos2(full.left() + nav_width, full.top()),
                egui::pos2(full.right(), full.bottom() - footer),
            );
            let foot = egui::Rect::from_min_max(egui::pos2(full.left(), full.bottom() - footer), full.max);
            ui.painter()
                .vline(nav.right(), nav.y_range(), egui::Stroke::new(1.0, theme.border));
            ui.painter()
                .hline(full.x_range(), foot.top(), egui::Stroke::new(1.0, theme.border));
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt("about-tabs")
                    .max_rect(nav.shrink2(egui::vec2(12.0, 0.0))),
                |ui| self.nav(ui, theme),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt("about-panel")
                    .max_rect(body.shrink2(egui::vec2(28.0, 0.0))),
                |ui| {
                    let tab = self.tab;
                    ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                        node.set_role(Role::TabPanel);
                        node.set_label(tab.label());
                    });
                    match tab {
                        AboutTab::About => about(ui, theme, info),
                        AboutTab::System => system(ui, theme, info),
                        AboutTab::Links => {
                            if let Some(url) = links_tab(ui, theme, links) {
                                result = AboutResult::OpenLink(url);
                            }
                        }
                    }
                },
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(foot.shrink2(egui::vec2(20.0, 12.0))),
                |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if dialogs::primary_button(ui, t(S::Done), theme).clicked() {
                            result = AboutResult::Closed;
                        }
                        if self.tab == AboutTab::System {
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| self.copy(ui, theme, info));
                        }
                    });
                },
            );
        });
        if result == AboutResult::Open && close.requested() {
            result = AboutResult::Closed;
        }
        result
    }

    /// The tab list: the window's title, then a row per tab.
    fn nav(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(Role::TabList);
            node.set_label(t(S::AboutWandur));
        });
        ui.add_space(24.0);
        ui.label(RichText::new(t(S::AboutWandur)).size(20.0).strong().color(theme.text));
        ui.add_space(18.0);
        let mut focused = None;
        for tab in AboutTab::ALL {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), Sense::hover());
            let response = ui.interact(rect, tab.id(), Sense::click());
            if response.has_focus() {
                focused = Some(tab);
                // Focus moved here from another tab (Up or Down): this tab is chosen.
                if self.focused.is_some_and(|f| f != tab) {
                    self.tab = tab;
                }
            }
            if response.clicked() {
                self.tab = tab;
            }
            let selected = self.tab == tab;
            if selected {
                ui.painter().rect_filled(rect, 3.0, theme.selection_fill());
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
            }
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect.shrink(1.0),
                    3.0,
                    egui::Stroke::new(1.5, theme.accent),
                    egui::StrokeKind::Inside,
                );
            }
            ui.painter().text(
                egui::pos2(rect.left() + 12.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                tab.label(),
                egui::FontId::proportional(14.0),
                theme.text,
            );
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_role(Role::Tab);
                node.set_label(tab.label());
                node.set_selected(selected);
            });
            ui.add_space(5.0);
        }
        self.focused = focused;
    }

    /// The footer's Copy (System information): the report on the clipboard, and Copied for a
    /// moment.
    fn copy(&mut self, ui: &mut Ui, theme: &Theme, info: &SystemInfo) {
        let now = ui.input(|i| i.time);
        let button = dialogs::secondary_button(ui, t(S::Copy));
        crate::a11y::label(&button, t(S::AboutCopySystemInfo));
        if button.clicked() {
            ui.ctx().copy_text(info.report());
            self.copied_at = Some(now);
        }
        if let Some(at) = self.copied_at {
            if now - at < COPIED_SECONDS {
                ui.add_space(6.0);
                ui.label(RichText::new(t(S::AboutCopied)).size(13.0).color(theme.ok));
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(COPIED_SECONDS - (now - at)));
            } else {
                self.copied_at = None;
            }
        }
    }
}

/// The System information tab: what it is for, then the facts in a box.
fn system(ui: &mut Ui, theme: &Theme, info: &SystemInfo) {
    heading(ui, AboutTab::System.label(), theme);
    ui.add(egui::Label::new(RichText::new(t(S::AboutSystemIntro)).size(13.0).color(theme.muted)).wrap());
    ui.add_space(14.0);
    let rows = info.rows(l10n::language());
    egui::ScrollArea::vertical()
        .id_salt("about-system-rows")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Frame::new()
                .fill(theme.shell)
                .stroke(egui::Stroke::new(1.0, theme.border))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let facts: Vec<Fact> = rows
                        .iter()
                        .map(|(label, value)| Fact {
                            label,
                            value,
                            note: None,
                        })
                        .collect();
                    fact_rows(ui, theme, &facts, true);
                });
            ui.add_space(12.0);
        });
}

/// A tab's heading, as the settings' sections have.
fn heading(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.add_space(24.0);
    ui.label(RichText::new(text).size(24.0).strong().color(theme.text));
    ui.add_space(10.0);
}

/// A row of [`fact_rows`]: a label, its value, and a muted line under the value.
struct Fact<'a> {
    label: &'a str,
    value: &'a str,
    note: Option<&'a str>,
}

/// Rows of a muted label and its value, the labels in one column as wide as the widest.
fn fact_rows(ui: &mut Ui, theme: &Theme, facts: &[Fact<'_>], selectable: bool) {
    let size = 13.0;
    let font = egui::FontId::proportional(size);
    let widest = facts
        .iter()
        .map(|f| {
            ui.painter()
                .layout_no_wrap(f.label.to_string(), font.clone(), theme.muted)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    let label_width = (widest + 18.0).min(ui.available_width() * 0.45);
    ui.spacing_mut().item_spacing.y = 7.0;
    for fact in facts {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_ui_with_layout(egui::vec2(label_width, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(label_width);
                ui.add(egui::Label::new(RichText::new(fact.label).size(size).color(theme.muted)).wrap());
            });
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.add(
                    egui::Label::new(RichText::new(fact.value).size(size).color(theme.text))
                        .wrap()
                        .selectable(selectable),
                );
                if let Some(note) = fact.note {
                    ui.add(egui::Label::new(RichText::new(note).size(12.0).color(theme.muted)).wrap());
                }
            });
        });
    }
}

/// The app icon at 80 points (`assets/icons/about-icon-160.png`).
fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = Id::new("wandur-about-logo");
    if let Some(handle) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(handle);
    }
    let bytes: &[u8] = include_bytes!("../assets/icons/about-icon-160.png");
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = image.to_rgba8();
    let color =
        egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
    let handle = ctx.load_texture("wandur-about-logo", color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    Some(handle)
}

/// The About tab: the logo beside the name, version and line about Wandur; then the studio,
/// licence, fonts and libraries.
fn about(ui: &mut Ui, theme: &Theme, info: &SystemInfo) {
    ui.add_space(28.0);
    let logo_size = 80.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(logo_size, logo_size), Sense::hover());
        if let Some(texture) = logo(ui.ctx()) {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        ui.vertical(|ui| {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(dialogs::DISPLAY_NAME)
                        .size(22.0)
                        .strong()
                        .color(theme.text),
                )
                .wrap(),
            );
            ui.add_space(4.0);
            let version = if info.commit == "unknown" {
                info.version.clone()
            } else {
                format!("{} ({})", info.version, info.commit)
            };
            ui.label(
                RichText::new(l10n::tf(S::AboutVersion, &[&version]))
                    .size(13.0)
                    .color(theme.muted),
            );
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(t(S::AboutTagline)).size(13.0).color(theme.text)).wrap());
        });
    });
    ui.add_space(22.0);
    ui.painter().hline(
        ui.max_rect().x_range(),
        ui.cursor().top(),
        egui::Stroke::new(1.0, theme.border),
    );
    ui.add_space(18.0);
    egui::ScrollArea::vertical()
        .id_salt("about-facts")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let fonts = FONTS
                .iter()
                .map(|(name, licence)| format!("{name} ({licence})"))
                .collect::<Vec<_>>()
                .join("\n");
            let licence = l10n::tf(S::AboutLicenceValue, &[&COPYRIGHT_YEAR, &STUDIO]);
            let fact = |label, value| Fact {
                label,
                value,
                note: None,
            };
            let facts = [
                fact(t(S::AboutStudio), STUDIO),
                fact(t(S::AboutLicence), &licence),
                fact(t(S::AboutFonts), &fonts),
                Fact {
                    note: Some(t(S::AboutLibrariesNote)),
                    ..fact(t(S::AboutBuiltWith), LIBRARIES)
                },
            ];
            fact_rows(ui, theme, &facts, false);
        });
}

/// The Links tab: a row per link, its title over its address. Returns the address chosen.
fn links_tab(ui: &mut Ui, theme: &Theme, links: &[Link]) -> Option<String> {
    heading(ui, AboutTab::Links.label(), theme);
    ui.add(egui::Label::new(RichText::new(t(S::AboutLinksIntro)).size(13.0).color(theme.muted)).wrap());
    ui.add_space(12.0);
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .id_salt("about-links")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for link in links {
                let title = t(link.title);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), Sense::hover());
                let response = ui.interact(rect, Id::new(("about-link", link.title)), Sense::click());
                if response.hovered() || response.has_focus() {
                    ui.painter().rect_filled(rect, 4.0, theme.hover_fill());
                }
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        rect.shrink(1.0),
                        4.0,
                        egui::Stroke::new(1.5, theme.accent),
                        egui::StrokeKind::Inside,
                    );
                }
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let text_left = rect.left() + 12.0;
                let arrow = 28.0;
                let width = (rect.width() - 24.0 - arrow).max(40.0);
                let title_galley =
                    crate::widgets::clipped(ui, title, egui::FontId::proportional(14.0), theme.accent, width, 1);
                ui.painter()
                    .galley(egui::pos2(text_left, rect.top() + 8.0), title_galley, theme.accent);
                let url_galley =
                    crate::widgets::clipped(ui, &link.url, egui::FontId::proportional(12.0), theme.muted, width, 1);
                ui.painter()
                    .galley(egui::pos2(text_left, rect.top() + 29.0), url_galley, theme.muted);
                let icon = egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 12.0 - arrow / 2.0 + 6.0, rect.center().y),
                    egui::vec2(14.0, 14.0),
                );
                paint_icon(ui, Icon::ChevronRight, icon, theme.muted);
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_role(Role::Link);
                    node.set_label(title);
                    node.set_url(link.url.as_str());
                });
                if response.clicked() {
                    chosen = Some(link.url.clone());
                }
            }
        });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SystemInfo {
        SystemInfo {
            version: "0.0.1".into(),
            commit: "80d51f9".into(),
            build: "debug, aarch64-apple-darwin".into(),
            os: "macOS 26.5".into(),
            arch: "aarch64".into(),
            renderer: None,
            scale: 2.0,
            data_dir: Some(mask_home(
                Path::new("/Users/pat/Library/Application Support/Wandur-Rust"),
                Some(Path::new("/Users/pat")),
            )),
            features: vec![("scripting", true), ("classifier", false)],
            sessions: 2,
            memory: Some(150 * 1024 * 1024),
        }
    }

    #[test]
    fn the_home_folder_shows_as_a_tilde() {
        let home = Some(Path::new("/Users/pat"));
        assert_eq!(mask_home(Path::new("/Users/pat"), home), "~");
        assert_eq!(
            mask_home(Path::new("/Users/pat/Library/Application Support/Wandur-Rust"), home),
            "~/Library/Application Support/Wandur-Rust"
        );
        // A sibling whose name starts the same is not under home.
        assert_eq!(mask_home(Path::new("/Users/patricia/w"), home), "/Users/patricia/w");
        assert_eq!(mask_home(Path::new("/srv/wandur"), home), "/srv/wandur");
        // Home inside a path that is not under it (a mount of it elsewhere) is still hidden.
        assert_eq!(mask_home(Path::new("/mnt/Users/pat/w"), home), "/mnt~/w");
        // No home, or a root home: the path as it is.
        assert_eq!(mask_home(Path::new("/srv/wandur"), None), "/srv/wandur");
        assert_eq!(mask_home(Path::new("/srv"), Some(Path::new("/"))), "/srv");
    }

    #[test]
    fn the_report_has_the_facts_in_english_and_nothing_else() {
        l10n::override_thread(Some(Language::De));
        let info = sample();
        let report = info.report();
        l10n::override_thread(None);
        assert!(
            report.starts_with("Wandur Mud Client (WMC) system information\n"),
            "{report}"
        );
        for line in [
            "Version: 0.0.1",
            "Commit: 80d51f9",
            "Build: debug, aarch64-apple-darwin",
            "Operating system: macOS 26.5",
            "Architecture: aarch64",
            "Renderer: not reported",
            "Display scale: 200%",
            "Data folder: ~/Library/Application Support/Wandur-Rust",
            "Features: +scripting -classifier",
            "Open sessions: 2",
            "Memory in use: 150 MB",
        ] {
            assert!(report.lines().any(|l| l == line), "{line} in\n{report}");
        }
        assert!(!report.contains("pat"), "{report}");
        assert_eq!(report.lines().count(), 12, "{report}");
    }

    /// The real gathering: this build's version and OS, the home folder masked, and nothing
    /// from the settings (a world's name, host or account) can be in it since it is not given
    /// any.
    #[test]
    fn gathered_information_is_this_build_with_home_masked() {
        let home = home_dir().expect("a home folder in the test environment");
        let info = SystemInfo::gather(Some("wgpu, Metal, Test GPU"), 1.5, Some(&home.join("wandur-data")), 3);
        let report = info.report();
        assert!(report.contains(&format!("Version: {VERSION}")), "{report}");
        assert!(
            report.contains(&format!("Operating system: {}", os_description())),
            "{report}"
        );
        assert!(!os_description().is_empty());
        assert!(report.contains("Data folder: ~"), "{report}");
        assert!(!report.contains(&home.display().to_string()), "{report}");
        assert!(report.contains("Renderer: wgpu, Metal, Test GPU"));
        assert!(report.contains("Display scale: 150%"));
        assert!(report.contains("Open sessions: 3"));
        assert!(report.contains(&format!("Architecture: {}", std::env::consts::ARCH)));
        assert!(!COMMIT.is_empty());
        if cfg!(target_os = "macos") {
            assert!(os_description().starts_with("macOS "), "{}", os_description());
        }
    }

    #[test]
    fn links_follow_the_site_and_the_repository() {
        let urls: Vec<String> = links("http://127.0.0.1:9/").into_iter().map(|l| l.url).collect();
        assert_eq!(
            urls,
            [
                "http://127.0.0.1:9/",
                "http://127.0.0.1:9/worlds",
                "http://127.0.0.1:9/clients",
                "https://github.com/Last-Mile-Studio/wandur",
                "https://github.com/Last-Mile-Studio/wandur/issues",
            ]
        );
        let public = links(wandur_core::site::PUBLIC_ADDRESS);
        assert_eq!(public[0].url, "https://www.wandur.net/");
        assert_eq!(public[2].url, "https://www.wandur.net/clients");
    }
}
