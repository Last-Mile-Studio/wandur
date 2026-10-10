//! Settings, as the C# client's preferences dialog: a modal window with its sections on the left
//! (General, Appearance, MUD colors, Terminal, Input, plus Connections for what only this client
//! has), the section on the right, and Cancel and Save preferences at the bottom.
//!
//! The dialog edits a copy of the settings. Save hands the copy back; Cancel drops it. A change
//! of language previews at once (every label redraws in the new language) and Cancel restores
//! the language that was in use, as in C#. Every C# setting has its control here.

use egui::{Align, Layout, RichText, Ui};
use wandur_core::l10n::{self, S, t, tf};
use wandur_core::settings::{
    ANSI_DEFAULTS, COLOR_KEYS, CustomTheme, FONT_SIZES, MAX_OUTPUT_FPS, MIN_SCROLLBACK, Settings, clamp_tail_share,
    is_color,
};

use crate::dialogs;
use crate::select::Select;
use crate::skin::SkinId;
use crate::theme::{Theme, mix, parse_hex, preset_colors, preset_label, to_hex};
use crate::widgets::{Icon, paint_icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Appearance,
    MudColors,
    Terminal,
    Input,
    Connections,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::General,
        Section::Appearance,
        Section::MudColors,
        Section::Terminal,
        Section::Input,
        Section::Connections,
    ];

    /// A section by the name scenes use (`general`, `mud-colors`, `terminal`, ...).
    pub fn from_name(name: &str) -> Option<Section> {
        Some(match name {
            "general" => Section::General,
            "appearance" => Section::Appearance,
            "mud-colors" => Section::MudColors,
            "terminal" => Section::Terminal,
            "input" => Section::Input,
            "connections" => Section::Connections,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        t(match self {
            Section::General => S::SettingsGeneral,
            Section::Appearance => S::SettingsAppearance,
            Section::MudColors => S::SettingsMudColors,
            Section::Terminal => S::SettingsTerminal,
            Section::Input => S::SettingsInput,
            Section::Connections => S::SettingsConnections,
        })
    }
}

/// What the dialog asks of the app this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsResult {
    Open,
    Cancelled,
    Saved(Box<Settings>),
    /// Put every panel back where it starts (Connections > Reset layout); the dialog stays open.
    ResetLayout,
}

pub struct SettingsDialog {
    /// The settings being edited.
    pub draft: Settings,
    /// The language setting when the dialog opened (Cancel goes back to it).
    original_language: String,
    pub section: Section,
    /// The sixteen colour boxes of MUD colors as typed (a half-typed colour is kept here, not in
    /// the draft), for the scheme they were loaded from.
    ansi_text: [String; 16],
    ansi_for: String,
    /// The Appearance page's colour boxes as typed, by [`COLOR_KEYS`], for `colors_for`.
    color_text: std::collections::BTreeMap<String, String>,
    colors_for: String,
    /// The Terminal page's colour boxes as typed.
    foreground: String,
    background: String,
    /// Why Save refused (shown in the footer).
    pub error: Option<String>,
    /// Hold the Keep history list open (scenes).
    pub open_retention: bool,
}

/// Switch the UI to a language setting (`""` follows the system).
pub fn apply_language(code: &str) {
    let system = sys_locale::get_locale();
    l10n::set_language(l10n::resolve(code, system.as_deref()));
}

impl SettingsDialog {
    pub fn new(settings: &Settings) -> Self {
        let mut dialog = Self {
            draft: settings.clone(),
            original_language: settings.language.clone(),
            section: Section::General,
            ansi_text: Default::default(),
            ansi_for: String::new(),
            color_text: Default::default(),
            colors_for: String::new(),
            foreground: settings.foreground.clone().unwrap_or_default(),
            background: settings.background.clone().unwrap_or_default(),
            error: None,
            open_retention: false,
        };
        dialog.load_palette();
        dialog
    }

    /// The scheme's sixteen colours and chrome colours into the colour boxes.
    fn load_palette(&mut self) {
        self.ansi_for = self.draft.theme.clone();
        let colors = scheme_ansi(&self.draft);
        self.ansi_text = colors;
        self.colors_for = self.draft.theme.clone();
        self.color_text = scheme_colors(&self.draft).0;
    }

    /// Whether the chosen scheme is one the person made (its colours can be edited).
    pub fn is_custom(&self) -> bool {
        self.draft.custom_theme().is_some()
    }

    /// Choose a colour scheme.
    pub fn choose_theme(&mut self, id: &str) {
        self.draft.theme = id.to_string();
        self.load_palette();
    }

    /// Create a copy of the chosen scheme and choose it (MUD colors > Create a copy).
    pub fn duplicate_theme(&mut self) {
        let (name, base) = match self.draft.custom_theme() {
            Some(custom) => (custom.name.clone(), custom.base.clone()),
            None => (
                Theme::preset(&self.draft.theme).name.to_string(),
                Theme::preset(&self.draft.theme).name.to_string(),
            ),
        };
        let short: String = name.chars().take(70).collect();
        let taken = |candidate: &str| {
            Theme::names().any(|n| n.eq_ignore_ascii_case(candidate))
                || self
                    .draft
                    .custom_themes
                    .iter()
                    .any(|t| t.name.eq_ignore_ascii_case(candidate))
        };
        let mut number = 1;
        let name = loop {
            let candidate = tf(S::ThemeCopyName, &[&short, &number]);
            if !taken(&candidate) {
                break candidate;
            }
            number += 1;
        };
        let (colors, is_light) = scheme_colors(&self.draft);
        let mut copy = CustomTheme {
            name,
            base,
            colors,
            is_light,
            ..CustomTheme::default()
        };
        copy.set_ansi(&scheme_ansi(&self.draft));
        let id = copy.id.clone();
        self.draft.custom_themes.push(copy);
        self.choose_theme(&id);
    }

    /// Set terminal colour `index` of the chosen custom scheme (`#RRGGBB`); ignored for presets
    /// and for text that is not a colour yet.
    pub fn set_ansi(&mut self, index: usize, hex: &str) {
        self.ansi_text[index] = hex.to_string();
        if !is_color(hex.trim()) {
            return;
        }
        let colors = self.ansi_text.clone().map(|c| c.trim().to_string());
        let theme = self.draft.theme.clone();
        if let Some(custom) = self.draft.custom_themes.iter_mut().find(|t| t.id == theme) {
            let mut current = custom.ansi();
            for (i, c) in colors.iter().enumerate() {
                if is_color(c) {
                    current[i] = c.clone();
                }
            }
            custom.set_ansi(&current);
        }
    }

    /// Delete the chosen custom scheme (Appearance > Delete theme); Ember is chosen instead,
    /// as in C#. Presets cannot be deleted.
    pub fn delete_theme(&mut self) {
        let id = self.draft.theme.clone();
        if !self.is_custom() {
            return;
        }
        self.draft.custom_themes.retain(|t| t.id != id);
        self.choose_theme("Ember");
    }

    /// Set chrome colour `key` of the chosen custom scheme; kept as typed until it is a colour.
    pub fn set_color(&mut self, key: &str, hex: &str) {
        self.color_text.insert(key.to_string(), hex.to_string());
        let hex = hex.trim();
        if !is_color(hex) {
            return;
        }
        let theme = self.draft.theme.clone();
        let (filled, light) = scheme_colors(&self.draft);
        if let Some(custom) = self.draft.custom_themes.iter_mut().find(|t| t.id == theme) {
            if custom.colors.is_empty() {
                // A scheme saved before it had chrome colours takes its base's first.
                custom.colors = filled;
                custom.is_light = light;
            }
            custom.colors.insert(key.to_string(), hex.to_uppercase());
        }
    }

    /// Rename the chosen custom scheme (checked for uniqueness on Save).
    pub fn set_theme_name(&mut self, name: &str) {
        let theme = self.draft.theme.clone();
        if let Some(custom) = self.draft.custom_themes.iter_mut().find(|t| t.id == theme) {
            custom.name = name.chars().take(100).collect();
        }
    }

    /// Light mode for the chosen custom scheme.
    pub fn set_theme_light(&mut self, light: bool) {
        let theme = self.draft.theme.clone();
        let (filled, _) = scheme_colors(&self.draft);
        if let Some(custom) = self.draft.custom_themes.iter_mut().find(|t| t.id == theme) {
            if custom.colors.is_empty() {
                custom.colors = filled;
            }
            custom.is_light = light;
        }
    }

    /// Save: the draft, if it passes the checks; otherwise the reason stays on screen.
    pub fn save(&mut self) -> SettingsResult {
        // A chrome colour box that does not hold #RRGGBB fails the custom scheme (C#: the hex
        // is what is saved, and it does not validate).
        if self.is_custom() && self.color_text.values().any(|c| !is_color(c.trim())) {
            self.error = Some(t(S::InvalidCustomTheme).into());
            return SettingsResult::Open;
        }
        for custom in &mut self.draft.custom_themes {
            custom.name = custom.name.trim().to_string();
        }
        self.draft.foreground = Some(self.foreground.trim().to_string()).filter(|c| !c.is_empty());
        self.draft.background = Some(self.background.trim().to_string()).filter(|c| !c.is_empty());
        match self.draft.validate_preferences() {
            Ok(()) => {
                self.error = None;
                SettingsResult::Saved(Box::new(self.draft.clone()))
            }
            Err(e) => {
                self.error = Some(e);
                SettingsResult::Open
            }
        }
    }

    /// Choose a language in the list: the UI previews it at once.
    pub fn choose_language(&mut self, code: &str) {
        self.draft.language = code.to_string();
        apply_language(code);
    }

    /// Close without saving: the language that was in use comes back.
    pub fn cancel(&self) -> SettingsResult {
        apply_language(&self.original_language);
        SettingsResult::Cancelled
    }

    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme, directory_base: &str) -> SettingsResult {
        let mut result = SettingsResult::Open;
        // The C# `OptionsDialog`: 880 by 740, at least 740 by 560, resizable, its own window.
        let spec = crate::dialog_window::Spec {
            id: "settings-dialog",
            title: t(S::SettingsTitle),
            default_size: egui::vec2(880.0, 740.0),
            min_size: egui::vec2(740.0, 560.0),
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
                egui::UiBuilder::new().max_rect(nav.shrink2(egui::vec2(12.0, 0.0))),
                |ui| self.nav(ui, theme),
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(28.0, 0.0))),
                |ui| self.body(ui, theme, directory_base, &mut result),
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(foot.shrink2(egui::vec2(20.0, 12.0))),
                |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if dialogs::primary_button(ui, t(S::SavePreferences), theme).clicked() {
                            result = self.save();
                        }
                        ui.add_space(4.0);
                        if dialogs::secondary_button(ui, t(S::Cancel2)).clicked() {
                            result = self.cancel();
                        }
                        if let Some(error) = &self.error {
                            ui.add_space(12.0);
                            ui.add(egui::Label::new(RichText::new(error).color(theme.error)).wrap());
                        }
                    });
                },
            );
        });
        if result == SettingsResult::Open && close.requested() {
            result = self.cancel();
        }
        result
    }

    fn nav(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.add_space(24.0);
        ui.label(RichText::new(t(S::SettingsTitle)).size(20.0).strong().color(theme.text));
        ui.add_space(18.0);
        for section in Section::ALL {
            let selected = self.section == section;
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
            if selected {
                ui.painter().rect_filled(rect, 3.0, theme.selection_fill());
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
            }
            ui.painter().text(
                egui::pos2(rect.left() + 12.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                section.label(),
                egui::FontId::proportional(14.0),
                theme.text,
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, section.label())
            });
            if response.clicked() {
                self.section = section;
            }
            ui.add_space(5.0);
        }
    }

    fn body(&mut self, ui: &mut Ui, theme: &Theme, directory_base: &str, result: &mut SettingsResult) {
        ui.add_space(24.0);
        ui.label(
            RichText::new(self.section.label())
                .size(24.0)
                .strong()
                .color(theme.text),
        );
        ui.add_space(14.0);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // The form keeps a reading measure (C# UI review, item 11: about 560 points).
            ui.set_width((ui.available_width() - 12.0).min(FORM_WIDTH));
            ui.spacing_mut().item_spacing.y = 6.0;
            // The C# check boxes: a 20 point box, 8 points before the label.
            ui.spacing_mut().icon_width = 20.0;
            ui.spacing_mut().icon_width_inner = 10.0;
            ui.spacing_mut().icon_spacing = 8.0;
            match self.section {
                Section::General => self.general(ui, theme),
                Section::Appearance => self.appearance(ui, theme),
                Section::MudColors => self.mud_colors(ui, theme),
                Section::Terminal => self.terminal(ui, theme),
                Section::Input => self.input(ui, theme),
                Section::Connections => self.connections(ui, theme, directory_base, result),
            }
            ui.add_space(12.0);
        });
    }

    fn general(&mut self, ui: &mut Ui, theme: &Theme) {
        field_label(ui, t(S::InterfaceLanguage), theme);
        let choices = l10n::choices();
        let mut chosen = None;
        let mut index = choices
            .iter()
            .position(|(code, _)| *code == self.draft.language)
            .unwrap_or(0);
        if Select::new("settings-language", t(S::InterfaceLanguage))
            .height(SELECT_H)
            .show_index(ui, &mut index, choices.len(), |i| choices[i].1)
            .changed()
        {
            chosen = Some(choices[index].0);
        }
        if let Some(code) = chosen {
            self.choose_language(code);
        }
        hint(ui, t(S::LanguageLiveHint), theme);
        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.show_channels, t(S::ShowChannelsPanel));
        hint(ui, t(S::ShowChannelsPanelHint), theme);
        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.check_for_updates, t(S::CheckForUpdatesAutomatically));
        hint(ui, t(S::CheckForUpdatesAutomaticallyHint), theme);
        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.send_install_id, t(S::SendInstallId));
        hint(ui, t(S::SendInstallIdHint), theme);
        ui.add_space(14.0);
        ui.label(
            RichText::new(t(S::SessionHistory))
                .size(17.0)
                .strong()
                .color(theme.text),
        );
        ui.add_space(4.0);
        // Applied when saved, never while previewing (C#).
        ui.checkbox(&mut self.draft.history_enabled, t(S::HistoryEnabled));
        hint(ui, t(S::HistoryPrivacyHint), theme);
        field_label(ui, t(S::HistoryRetention), theme);
        let retention = |days: u32| match days {
            30 => t(S::HistoryDays30),
            90 => t(S::HistoryDays90),
            365 => t(S::HistoryDays365),
            _ => t(S::HistoryForever),
        };
        let choices = wandur_core::history::RETENTION_CHOICES;
        let mut index = choices
            .iter()
            .position(|&d| d == self.draft.history_retention_days)
            .unwrap_or(choices.len() - 1);
        if self.open_retention {
            // The list opens below the box; keep both on screen.
            ui.scroll_to_cursor(Some(Align::Max));
        }
        if Select::new("settings-history", t(S::HistoryRetention))
            .height(SELECT_H)
            .open_now(self.open_retention)
            .show_index(ui, &mut index, choices.len(), |i| retention(choices[i]))
            .changed()
        {
            self.draft.history_retention_days = choices[index];
        }
    }

    /// Appearance, as the C# page: the skin, the colour scheme with Create a copy and Delete
    /// theme, the world themes switch, and the scheme's chrome colours (editable in a copy).
    /// Every change previews at once; Cancel puts the old appearance back.
    fn appearance(&mut self, ui: &mut Ui, theme: &Theme) {
        if self.colors_for != self.draft.theme {
            self.load_palette();
        }
        ui.spacing_mut().item_spacing.y = 8.0;
        field_label(ui, t(S::Skin), theme);
        let current = SkinId::from_name(&self.draft.skin);
        let mut index = SkinId::ALL.iter().position(|&id| id == current).unwrap_or(0);
        if Select::new("settings-skin", t(S::Skin))
            .height(SELECT_H)
            .show_index(ui, &mut index, SkinId::ALL.len(), |i| SkinId::ALL[i].label())
            .changed()
        {
            self.draft.skin = SkinId::ALL[index].name().into();
        }
        hint(ui, t(S::SkinHelp), theme);
        ui.add_space(4.0);
        field_label(ui, t(S::ColorScheme), theme);
        let chosen = scheme_combo(ui, "settings-theme", &self.draft);
        if let Some(id) = chosen {
            self.choose_theme(&id);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if dialogs::secondary_button(ui, t(S::DuplicateTheme)).clicked() {
                self.duplicate_theme();
            }
            let custom = self.is_custom();
            if ui
                .add_enabled_ui(custom, |ui| dialogs::secondary_button(ui, t(S::DeleteTheme)))
                .inner
                .clicked()
            {
                self.delete_theme();
            }
        });
        hint(ui, t(S::PresetThemeHint), theme);
        ui.add_space(4.0);
        ui.checkbox(
            &mut self.draft.use_world_themes,
            RichText::new(t(S::UseWorldThemes)).size(14.0),
        );
        if self.draft.use_world_themes {
            hint(ui, t(S::WorldThemePriority), theme);
        }
        let custom = self.is_custom();
        if let Some(scheme) = self.draft.custom_theme().cloned() {
            field_label(ui, t(S::ThemeNameLabel), theme);
            let mut name = scheme.name.clone();
            let field = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .char_limit(100)
                    .desired_width(ui.available_width())
                    .margin(egui::Margin::symmetric(10, 7)),
            );
            crate::a11y::label(&field, t(S::ThemeNameLabel));
            if field.changed() {
                self.set_theme_name(&name);
            }
            let mut light = if scheme.colors.is_empty() {
                scheme_colors(&self.draft).1
            } else {
                scheme.is_light
            };
            if ui.checkbox(&mut light, t(S::ThemeLightMode)).changed() {
                self.set_theme_light(light);
            }
        }
        ui.add_space(8.0);
        for key in COLOR_KEYS {
            let label = palette_label(key);
            let mut edited = None;
            ui.horizontal(|ui| {
                ui.set_min_height(34.0);
                ui.label(RichText::new(label).size(14.0).color(theme.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let mut text = self.color_text.get(key).cloned().unwrap_or_default();
                    let field = ui.add_enabled(
                        custom,
                        egui::TextEdit::singleline(&mut text)
                            .font(egui::FontId::monospace(14.0))
                            .char_limit(7)
                            .desired_width(100.0)
                            .margin(egui::Margin::symmetric(8, 7)),
                    );
                    crate::a11y::label(&field, label);
                    if field.changed() {
                        edited = Some(text);
                    }
                    let shown = self.color_text.get(key).and_then(|c| parse_hex(c));
                    let mut color = shown.unwrap_or(egui::Color32::BLACK);
                    let before = color;
                    ui.add_enabled_ui(custom, |ui| {
                        swatch(ui, &mut color, theme, label).on_hover_text(label);
                    });
                    if color != before {
                        edited = Some(to_hex(color));
                    }
                });
            });
            if let Some(hex) = edited {
                self.set_color(key, &hex);
            }
        }
    }

    /// MUD colors: the scheme, Create a copy, and its sixteen terminal colours (editable in a
    /// copy), as the C# page.
    fn mud_colors(&mut self, ui: &mut Ui, theme: &Theme) {
        if self.ansi_for != self.draft.theme {
            self.load_palette();
        }
        field_label(ui, t(S::ColorScheme), theme);
        let chosen = scheme_combo(ui, "settings-ansi-theme", &self.draft);
        if let Some(id) = chosen {
            self.choose_theme(&id);
        }
        ui.add_space(6.0);
        if dialogs::secondary_button(ui, t(S::DuplicateTheme)).clicked() {
            self.duplicate_theme();
        }
        ui.add_space(4.0);
        ui.label(RichText::new(t(S::PresetThemeHint)).size(12.0).color(theme.muted));
        ui.add_space(4.0);
        hint(ui, t(S::AnsiPaletteHint), theme);
        ui.add_space(6.0);
        let custom = self.is_custom();
        let labels = [
            S::AnsiBlack,
            S::AnsiRed,
            S::AnsiGreen,
            S::AnsiYellow,
            S::AnsiBlue,
            S::AnsiMagenta,
            S::AnsiCyan,
            S::AnsiWhite,
            S::AnsiBrightBlack,
            S::AnsiBrightRed,
            S::AnsiBrightGreen,
            S::AnsiBrightYellow,
            S::AnsiBrightBlue,
            S::AnsiBrightMagenta,
            S::AnsiBrightCyan,
            S::AnsiBrightWhite,
        ];
        for (i, label) in labels.into_iter().enumerate() {
            let mut edited = None;
            ui.horizontal(|ui| {
                ui.set_min_height(34.0);
                ui.label(RichText::new(t(label)).size(14.0).color(theme.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let reset = ui.add_enabled(
                        custom,
                        egui::Button::new(RichText::new(t(S::ResetAnsiColor)).size(13.0))
                            .min_size(egui::vec2(58.0, 32.0)),
                    );
                    if reset.clicked() {
                        edited = Some(ANSI_DEFAULTS[i].to_string());
                    }
                    let mut text = self.ansi_text[i].clone();
                    let field = ui.add_enabled(
                        custom,
                        egui::TextEdit::singleline(&mut text)
                            .font(egui::FontId::monospace(14.0))
                            .char_limit(7)
                            .desired_width(92.0)
                            .margin(egui::Margin::symmetric(8, 7)),
                    );
                    crate::a11y::label(&field, t(label));
                    if field.changed() {
                        edited = Some(text);
                    }
                    let mut color = parse_hex(&self.ansi_text[i]).unwrap_or(egui::Color32::GRAY);
                    let before = color;
                    ui.add_enabled_ui(custom, |ui| {
                        swatch(ui, &mut color, theme, t(label)).on_hover_text(t(label));
                    });
                    if color != before {
                        edited = Some(to_hex(color));
                    }
                });
            });
            if let Some(hex) = edited {
                self.set_ansi(i, &hex);
            }
        }
    }

    fn terminal(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.checkbox(&mut self.draft.allow_blinking_text, t(S::AllowBlinkingMudText));
        ui.checkbox(&mut self.draft.wrap_words, t(S::WrapWords));
        ui.add_enabled_ui(self.draft.wrap_words, |ui| {
            ui.checkbox(&mut self.draft.wrap_indent, t(S::WrapIndent));
        });
        hint(ui, t(S::WrapWordsHint), theme);
        ui.add_space(4.0);
        ui.checkbox(&mut self.draft.limit_text_width, t(S::LimitTextWidth));
        ui.add_enabled_ui(self.draft.limit_text_width, |ui| {
            let mut columns = self.draft.text_width_columns as i64;
            let range = wandur_core::settings::TEXT_WIDTH_COLUMNS;
            if spin_box(
                ui,
                "settings-text-width",
                t(S::LimitTextWidthColumns),
                &mut columns,
                *range.start() as i64..=*range.end() as i64,
                10,
                "",
                theme,
            ) {
                self.draft.text_width_columns = columns as u32;
            }
        });
        hint(ui, t(S::LimitTextWidthHint), theme);
        ui.add_space(4.0);
        field_label(ui, t(S::SessionTextSize), theme);
        let mut size = self.draft.font_size.round() as i64;
        if spin_box(
            ui,
            "settings-text-size",
            t(S::SessionTextSize),
            &mut size,
            *FONT_SIZES.start() as i64..=*FONT_SIZES.end() as i64,
            1,
            "",
            theme,
        ) {
            self.draft.font_size = size as f32;
        }
        ui.add_space(6.0);
        // The preview: the transcript's colours (with the overrides typed here) at the size chosen.
        let preview = Theme::from_settings(&Settings {
            foreground: Some(self.foreground.trim().to_string()).filter(|c| is_color(c)),
            background: Some(self.background.trim().to_string()).filter(|c| is_color(c)),
            ..self.draft.clone()
        });
        egui::Frame::new()
            .fill(preview.terminal)
            .corner_radius(4)
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(t(S::TheLanternCastsAWarmLightExitsNorthEast))
                        .font(egui::FontId::monospace(self.draft.font_size))
                        .color(preview.terminal_text),
                );
            });
        ui.add_space(6.0);
        field_label(ui, t(S::LiveViewWhileScrolledUp), theme);
        let mut percent = (self.draft.scroll_tail_share * 100.0).round() as i64;
        if spin_box(
            ui,
            "settings-tail-share",
            t(S::LiveViewWhileScrolledUp),
            &mut percent,
            0..=60,
            5,
            "%",
            theme,
        ) {
            self.draft.scroll_tail_share = clamp_tail_share(percent as f32 / 100.0);
        }
        hint(ui, t(S::ShareOfTheWindowThatKeepsScrolling), theme);
        hint(ui, t(S::TextColorOptionalRRGGBB), theme);
        color_box(ui, &mut self.foreground, t(S::TextColorOptionalRRGGBB), theme);
        hint(ui, t(S::BackgroundOptionalRRGGBB), theme);
        color_box(ui, &mut self.background, t(S::BackgroundOptionalRRGGBB), theme);
    }

    fn input(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.checkbox(&mut self.draft.local_echo, t(S::ShowMyCommandsInTheTranscript));
        ui.add_space(4.0);
        hint(ui, t(S::LocalEchoStartsOffUsePrivateInputForPasswords), theme);
        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.composer_suggestions, t(S::ComposerSuggestions));
        ui.add_space(4.0);
        hint(ui, t(S::ComposerSuggestionsHint), theme);
        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.enable_lua_scripts, t(S::PreferencesEnableLua));
        ui.add_space(4.0);
        hint(ui, t(S::PreferencesEnableLuaHint), theme);
    }

    fn connections(&mut self, ui: &mut Ui, theme: &Theme, directory_base: &str, result: &mut SettingsResult) {
        // What only this client has: scrollback and the output frame cap (from the old
        // Terminal page), reconnecting, prompts, the directory and the layout.
        field_label(ui, t(S::ScrollbackRows), theme);
        let rows = ui.add(
            egui::DragValue::new(&mut self.draft.scrollback)
                .range(MIN_SCROLLBACK..=wandur_term::MAX_SCROLLBACK)
                .speed(50.0),
        );
        crate::a11y::label(&rows, t(S::ScrollbackRows));
        hint(ui, &tf(S::ScrollbackRowsHint, &[&wandur_term::MAX_SCROLLBACK]), theme);
        field_label(ui, t(S::OutputFramesASecond), theme);
        let fps = ui.add(egui::Slider::new(&mut self.draft.output_fps, 0..=MAX_OUTPUT_FPS));
        crate::a11y::label(&fps, t(S::OutputFramesASecond));
        hint(ui, t(S::OutputFramesASecondHint), theme);
        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.auto_reconnect, t(S::ReconnectAutomatically));
        hint(ui, t(S::ReconnectAutomaticallyGlobalHint), theme);
        field_label(ui, t(S::PromptAfterAQuietLine), theme);
        let quiet = ui.add(
            egui::DragValue::new(&mut self.draft.prompt_quiet_ms)
                .range(50..=5_000)
                .speed(10.0)
                .suffix(" ms"),
        );
        crate::a11y::label(&quiet, t(S::PromptAfterAQuietLine));
        ui.add_space(8.0);
        field_label(ui, t(S::DirectoryAddress), theme);
        let address = ui.add(
            egui::TextEdit::singleline(&mut self.draft.directory_url)
                .hint_text(wandur_core::directory::client::PUBLIC_DIRECTORY)
                .desired_width(ui.available_width()),
        );
        crate::a11y::label(&address, t(S::DirectoryAddress));
        let env = std::env::var("WANDUR_DIRECTORY_URL")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let note = match env {
            Some(v) => tf(S::DirectoryInUseOverridden, &[&directory_base, &v]),
            None => tf(S::DirectoryInUse, &[&directory_base]),
        };
        hint(ui, &note, theme);
        ui.add_space(8.0);
        ui.checkbox(&mut self.draft.show_adult, t(S::AdultWorlds));
        hint(ui, t(S::AdultWorldsHint), theme);
        ui.add_space(8.0);
        if ui.button(t(S::RestorePanels)).clicked() {
            *result = SettingsResult::ResetLayout;
        }
        hint(ui, t(S::LayoutSavedHint), theme);
    }
}

fn field_label(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).size(14.0).color(theme.text));
}

/// The widest a settings page's form gets.
const FORM_WIDTH: f32 = 560.0;
/// The height of the selects on these pages.
const SELECT_H: f32 = 32.0;

fn hint(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(RichText::new(text).size(13.0).color(theme.muted));
}

/// The colour scheme list: every preset, then the custom schemes. Returns the one picked.
fn scheme_combo(ui: &mut Ui, id: &str, settings: &Settings) -> Option<String> {
    let presets: Vec<&str> = Theme::names().collect();
    let count = presets.len() + settings.custom_themes.len();
    let id_of = |i: usize| -> &str {
        if i < presets.len() {
            presets[i]
        } else {
            &settings.custom_themes[i - presets.len()].id
        }
    };
    let mut index = (0..count).position(|i| id_of(i) == settings.theme).unwrap_or(0);
    let before = index;
    let response = Select::new(id, t(S::ColorScheme))
        .height(SELECT_H)
        .show_index(ui, &mut index, count, |i| {
            if i < presets.len() {
                preset_label(presets[i])
            } else {
                settings.custom_themes[i - presets.len()].name.as_str()
            }
        });
    (response.changed() && index != before && index < count).then(|| id_of(index).to_string())
}

/// The chosen scheme's chrome colours and lightness: a custom scheme's own, else its base
/// preset's (or the preset's).
fn scheme_colors(settings: &Settings) -> (std::collections::BTreeMap<String, String>, bool) {
    match settings.custom_theme() {
        Some(custom) if !custom.colors.is_empty() => (custom.colors.clone(), custom.is_light),
        Some(custom) => preset_colors(&custom.base),
        None => preset_colors(&settings.theme),
    }
}

/// A chrome colour row's label (the C# `ThemeColorViewModel.Label`).
fn palette_label(key: &str) -> &'static str {
    t(match key {
        "Shell" => S::PaletteShell,
        "Panel" => S::PalettePanel,
        "Terminal" => S::PaletteTerminal,
        "Text" => S::PaletteText,
        "Muted" => S::PaletteMuted,
        "Accent" => S::PaletteAccent,
        "AccentSecondary" => S::PaletteAccentSecondary,
        "Border" => S::PaletteBorder,
        "TerminalText" => S::PaletteTerminalText,
        "Chrome" => S::PaletteChrome,
        "Selection" => S::PaletteSelection,
        "Button" => S::PaletteButton,
        "ButtonText" => S::PaletteButtonText,
        "PrimaryText" => S::PalettePrimaryText,
        "MapBackground" => S::PaletteMapBackground,
        "MapGrid" => S::PaletteMapGrid,
        "EditorBackground" => S::PaletteEditorBackground,
        _ => S::PaletteEditorText,
    })
}

/// The chosen scheme's sixteen terminal colours as `#RRGGBB`.
fn scheme_ansi(settings: &Settings) -> [String; 16] {
    match settings.custom_theme() {
        Some(custom) => custom.ansi(),
        None => Theme::preset(&settings.theme).ansi.map(to_hex),
    }
}

/// A colour swatch that opens egui's colour picker (the C# row's colour button).
fn swatch(ui: &mut Ui, color: &mut egui::Color32, theme: &Theme, label: &str) -> egui::Response {
    let popup_id = ui.auto_id_with("swatch");
    let (rect, response) = ui.allocate_exact_size(egui::vec2(62.0, 32.0), egui::Sense::click());
    crate::a11y::control(&response, egui::accesskit::Role::ColorWell, label);
    let enabled = ui.is_enabled();
    let frame = if response.hovered() && enabled {
        mix(theme.panel, theme.text, 0.18)
    } else {
        mix(theme.panel, theme.text, 0.10)
    };
    ui.painter().rect_filled(rect, 3.0, frame);
    let chip = egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(28.0, 24.0));
    ui.painter().rect_filled(chip, 2.0, *color);
    let arrow = egui::Rect::from_min_max(egui::pos2(chip.right(), rect.top()), rect.max);
    paint_icon(ui, Icon::ChevronDown, arrow, theme.muted);
    let mut changed = false;
    egui::Popup::from_toggle_button_response(&response)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            changed = egui::color_picker::color_picker_color32(ui, color, egui::color_picker::Alpha::Opaque);
        });
    let mut response = response;
    if changed {
        response.mark_changed();
    }
    response
}

/// A whole-width number box with up and down buttons (the C# `NumericUpDown`). Returns whether
/// the value changed.
#[allow(clippy::too_many_arguments)]
fn spin_box(
    ui: &mut Ui,
    id: &str,
    label: &str,
    value: &mut i64,
    range: std::ops::RangeInclusive<i64>,
    step: i64,
    suffix: &str,
    theme: &Theme,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let width = ui.available_width() - 70.0;
        // While the box is being typed in, the text is kept as typed; it counts when the box
        // loses focus (Enter included).
        let edit_id = egui::Id::new(id);
        let typing = ui.memory(|m| m.has_focus(edit_id));
        let mut text = typing
            .then(|| ui.data(|d| d.get_temp::<String>(edit_id.with("text"))))
            .flatten()
            .unwrap_or_else(|| format!("{value}{suffix}"));
        let field = ui.add(
            egui::TextEdit::singleline(&mut text)
                .id(edit_id)
                .desired_width(width)
                .margin(egui::Margin::symmetric(10, 7)),
        );
        crate::a11y::label(&field, label);
        if field.has_focus() {
            ui.data_mut(|d| d.insert_temp(edit_id.with("text"), text.clone()));
        }
        if field.lost_focus() {
            let digits: String = text.chars().filter(char::is_ascii_digit).collect();
            if let Ok(v) = digits.parse::<i64>() {
                *value = v.clamp(*range.start(), *range.end());
            }
        }
        for (icon, delta) in [(Icon::ChevronUp, step), (Icon::ChevronDown, -step)] {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(35.0, 32.0), egui::Sense::click());
            let fill = if response.hovered() {
                mix(theme.panel, theme.text, 0.14)
            } else {
                mix(theme.panel, egui::Color32::WHITE, 0.4)
            };
            ui.painter().rect_filled(rect, 0.0, fill);
            ui.painter().rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, mix(theme.panel, theme.text, 0.2)),
                egui::StrokeKind::Inside,
            );
            paint_icon(ui, icon, rect, theme.text);
            if response.clicked() {
                *value = (*value + delta).clamp(*range.start(), *range.end());
            }
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, if delta > 0 { "+" } else { "-" })
            });
        }
    });
    *value != before
}

/// A colour box with "Use scheme default" when empty.
fn color_box(ui: &mut Ui, text: &mut String, label: &str, theme: &Theme) {
    let field = ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(RichText::new(t(S::UseSchemeDefault)).color(theme.muted))
            .desired_width(ui.available_width())
            .margin(egui::Margin::symmetric(10, 7)),
    );
    crate::a11y::label(&field, label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_core::l10n::{Language, language, override_thread};

    #[test]
    fn choosing_a_language_previews_it_and_cancel_restores_the_old_one() {
        override_thread(Some(Language::En));
        let settings = Settings {
            language: "en".into(),
            ..Settings::default()
        };
        let mut dialog = SettingsDialog::new(&settings);
        dialog.choose_language("de");
        assert_eq!(language(), Language::De);
        assert_eq!(Section::General.label(), "Allgemein");
        assert_eq!(dialog.cancel(), SettingsResult::Cancelled);
        assert_eq!(language(), Language::En);
        override_thread(None);
    }

    /// Settings > General: the history rows edit the draft, and the Keep history list offers the
    /// four C# choices (opened as the reference capture shows it).
    #[test]
    fn history_rows_edit_the_draft_and_the_retention_list_opens() {
        override_thread(Some(Language::En));
        let mut dialog = SettingsDialog::new(&Settings::default());
        dialog.open_retention = true;
        let ctx = egui::Context::default();
        let theme = Theme::default();
        let mut open = false;
        let mut texts = Vec::new();
        for _ in 0..4 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = dialog.show(ui.ctx(), &theme, "");
            });
            output.textures_delta.clear();
            open = egui::Popup::is_any_open(&ctx);
            texts = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
        }
        assert!(open, "the list is open");
        for label in ["30 days", "90 days", "1 year", "Forever"] {
            assert!(texts.iter().any(|t| t == label), "{label} in {texts:?}");
        }
        dialog.draft.history_retention_days = 365;
        dialog.draft.history_enabled = false;
        match dialog.save() {
            SettingsResult::Saved(saved) => {
                assert_eq!(saved.history_retention_days, 365);
                assert!(!saved.history_enabled);
            }
            other => panic!("{other:?}"),
        }
        override_thread(None);
    }

    #[test]
    fn every_section_has_a_label_in_every_language() {
        for language in Language::ALL {
            override_thread(Some(language));
            for section in Section::ALL {
                assert!(!section.label().is_empty());
            }
        }
        override_thread(None);
    }

    /// MUD colors: a preset is read only; Create a copy makes an editable scheme named
    /// "Hull copy 1" with the preset's colours; edits keep only what differs from the defaults;
    /// Reset goes back to the default; Save refuses a colour that is not #RRGGBB.
    #[test]
    fn the_appearance_editor_edits_a_copy_and_checks_hex_and_names() {
        override_thread(Some(Language::En));
        let mut dialog = SettingsDialog::new(&Settings::default());
        // A preset's colours are shown, not editable.
        assert_eq!(dialog.color_text.get("Shell").map(String::as_str), Some("#D1D4D2"));
        assert_eq!(dialog.color_text.len(), COLOR_KEYS.len());
        dialog.set_color("Panel", "#123456");
        assert!(dialog.draft.custom_themes.is_empty(), "a preset is not edited");
        dialog.duplicate_theme();
        let copy = dialog.draft.custom_theme().unwrap().clone();
        assert_eq!(copy.colors.len(), COLOR_KEYS.len(), "a copy names every chrome colour");
        assert!(copy.is_light);
        assert_eq!(Theme::from_settings(&dialog.draft).panel, Theme::preset("Hull").panel);
        // Edits preview at once (the app draws the draft while the dialog is open).
        dialog.set_color("Panel", "#123456");
        dialog.set_theme_light(false);
        let preview = Theme::from_settings(&dialog.draft);
        assert_eq!(preview.panel, parse_hex("#123456").unwrap());
        assert!(!preview.light);
        // A half-typed colour keeps the last good one, and Save refuses it.
        dialog.set_color("Text", "#12");
        assert_eq!(dialog.draft.custom_theme().unwrap().color("Text"), Some("#202629"));
        assert_eq!(dialog.save(), SettingsResult::Open);
        assert_eq!(dialog.error.as_deref(), Some(t(S::InvalidCustomTheme)));
        dialog.set_color("Text", "#f0f0f0");
        assert_eq!(dialog.draft.custom_theme().unwrap().color("Text"), Some("#F0F0F0"));
        // A name a preset has is refused; a fresh one saves.
        dialog.set_theme_name("slate");
        assert_eq!(dialog.save(), SettingsResult::Open);
        assert_eq!(dialog.error.as_deref(), Some(t(S::ThemeNameUnique)));
        dialog.set_theme_name("  Night watch  ");
        let saved = match dialog.save() {
            SettingsResult::Saved(settings) => settings,
            other => panic!("{other:?}"),
        };
        let theme = saved.custom_theme().unwrap();
        assert_eq!(theme.name, "Night watch");
        assert!(theme.is_valid());
        // The skin and world themes are part of the page too.
        dialog.draft.skin = "Armored".into();
        dialog.draft.use_world_themes = false;
        // Delete theme: the copy goes and Ember is chosen, as in C#.
        dialog.delete_theme();
        assert!(dialog.draft.custom_themes.is_empty());
        assert_eq!(dialog.draft.theme, "Ember");
        dialog.delete_theme();
        assert_eq!(dialog.draft.theme, "Ember", "a preset cannot be deleted");
        match dialog.save() {
            SettingsResult::Saved(settings) => {
                assert_eq!(settings.skin, "Armored");
                assert!(!settings.use_world_themes);
            }
            other => panic!("{other:?}"),
        }
        override_thread(None);
    }

    /// A scheme saved before it had chrome colours takes its base preset's on its first edit.
    #[test]
    fn an_older_copy_gets_its_base_colours_on_its_first_edit() {
        let old = CustomTheme {
            name: "Old copy".into(),
            base: "Slate".into(),
            ..CustomTheme::default()
        };
        let settings = Settings {
            theme: old.id.clone(),
            custom_themes: vec![old],
            ..Settings::default()
        };
        let mut dialog = SettingsDialog::new(&settings);
        assert_eq!(dialog.color_text.get("Panel").map(String::as_str), Some("#212428"));
        dialog.set_color("Accent", "#FF8800");
        let custom = dialog.draft.custom_theme().unwrap();
        assert_eq!(custom.colors.len(), COLOR_KEYS.len());
        assert_eq!(custom.color("Panel"), Some("#212428"));
        assert!(!custom.is_light);
        assert_eq!(
            Theme::from_settings(&dialog.draft).accent,
            parse_hex("#FF8800").unwrap()
        );
    }

    #[test]
    fn a_copy_of_a_scheme_takes_edits_and_save_checks_the_terminal_colours() {
        override_thread(Some(Language::En));
        let mut dialog = SettingsDialog::new(&Settings::default());
        assert!(!dialog.is_custom());
        dialog.set_ansi(1, "#112233");
        assert!(dialog.draft.custom_themes.is_empty(), "a preset is not edited");
        dialog.duplicate_theme();
        assert!(dialog.is_custom());
        let custom = dialog.draft.custom_theme().unwrap().clone();
        assert_eq!(custom.name, "Hull copy 1");
        assert_eq!(custom.base, "Hull");
        assert_eq!(custom.ansi()[0], "#6B6B80", "the preset's own black");
        dialog.set_ansi(1, "#1122");
        assert_eq!(
            dialog.draft.custom_theme().unwrap().ansi()[1],
            "#DE6363",
            "half typed: unchanged"
        );
        dialog.set_ansi(1, "#112233");
        assert_eq!(dialog.draft.custom_theme().unwrap().ansi()[1], "#112233");
        dialog.set_ansi(1, ANSI_DEFAULTS[1]);
        assert!(!dialog.draft.custom_theme().unwrap().ansi_colors.contains_key(&1));
        // A second copy gets the next number.
        dialog.choose_theme("Hull");
        dialog.duplicate_theme();
        assert_eq!(dialog.draft.custom_theme().unwrap().name, "Hull copy 2");

        dialog.foreground = "red".into();
        assert_eq!(dialog.save(), SettingsResult::Open);
        assert!(dialog.error.is_some());
        dialog.foreground = "#A0B0C0".into();
        match dialog.save() {
            SettingsResult::Saved(settings) => {
                assert_eq!(settings.foreground.as_deref(), Some("#A0B0C0"));
                assert_eq!(settings.custom_themes.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        override_thread(None);
    }
}
