//! Small modal dialogs drawn over the dimmed window, as the C# client shows its dialogs: About
//! Wandur, the "Open this link?" confirmation, an information box (Check for Updates), and File > Import from Mudlet (the chooser and
//! the summary, the C# `MainWindow.MudletImport`).

use egui::{Align, Layout, RichText, Ui};
use wandur_core::l10n::{S, t, tf};

use crate::theme::Theme;

/// The client's display name (the C# `ClientIdentity.DisplayName`).
pub const DISPLAY_NAME: &str = "Wandur Mud Client (WMC)";
/// The app's name in window titles (C# `MainWindow.AppName`).
pub const APP_NAME: &str = "Wandur Mud Client";

/// What a dialog asked for when it closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogResult {
    Open,
    Closed,
    /// The link dialog's Open: open this address.
    OpenLink(String),
    /// The Mudlet chooser's choice: pick a profile folder, or a profile or package file.
    MudletPick(MudletPick),
}

/// What the Mudlet chooser asks the person to pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MudletPick {
    Folder,
    File,
}

/// A dialog on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    About,
    /// Confirm before opening `url` in the browser.
    Link(String),
    /// Import from Mudlet: what can be imported, and where a package goes (the selected world's
    /// name, or none).
    MudletChooser {
        target: Option<String>,
    },
    /// Import finished: the summary text.
    MudletSummary(String),
    /// A heading and a message with Done (the C# `ShowInformationAsync`): Check for Updates'
    /// answer.
    Information {
        heading: String,
        message: String,
    },
}

/// A modal frame of `width` with the theme's panel colour over the dimmed window.
pub fn modal<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    theme: &Theme,
    add: impl FnOnce(&mut Ui) -> R,
) -> egui::ModalResponse<R> {
    egui::Modal::new(egui::Id::new(id))
        .backdrop_color(theme.dim())
        .frame(
            egui::Frame::new()
                .fill(theme.panel)
                .stroke(egui::Stroke::new(1.0, theme.border))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(28, 22)),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            add(ui)
        })
}

/// A filled accent button, the C# dialogs' default button.
pub fn primary_button(ui: &mut Ui, text: &str, theme: &Theme) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(14.0).strong().color(theme.on_primary()))
            .fill(theme.primary())
            .min_size(egui::vec2(60.0, 32.0)),
    )
}

/// A filled accent button at the size of an inline prompt's buttons.
pub fn primary_button_small(ui: &mut Ui, text: &str, theme: &Theme) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(12.0).strong().color(theme.on_primary())).fill(theme.primary()))
}

/// A plain button of the same size.
pub fn secondary_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(14.0)).min_size(egui::vec2(60.0, 32.0)))
}

impl Dialog {
    pub fn show(&self, ctx: &egui::Context, theme: &Theme) -> DialogResult {
        match self {
            Dialog::About => about(ctx, theme),
            Dialog::Link(url) => link(ctx, url, theme),
            Dialog::MudletChooser { target } => mudlet_chooser(ctx, target.as_deref(), theme),
            Dialog::MudletSummary(text) => mudlet_summary(ctx, text, theme),
            Dialog::Information { heading, message } => information(ctx, heading, message, theme),
        }
    }
}

/// 480 wide with 28 padding: the heading at 24, the message muted at 13, Done.
fn information(ctx: &egui::Context, heading: &str, message: &str, theme: &Theme) -> DialogResult {
    let response = modal(ctx, "information", 424.0, theme, |ui| {
        ui.label(RichText::new(heading).size(24.0).color(theme.text));
        if !message.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(message).size(13.0).color(theme.muted));
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            primary_button(ui, t(S::Done), theme).clicked()
        })
        .inner
    });
    if response.inner || response.should_close() {
        DialogResult::Closed
    } else {
        DialogResult::Open
    }
}

fn about(ctx: &egui::Context, theme: &Theme) -> DialogResult {
    let response = modal(ctx, "about", 424.0, theme, |ui| {
        ui.label(RichText::new(DISPLAY_NAME).size(25.0).color(theme.text));
        ui.add_space(12.0);
        ui.label(
            RichText::new(t(S::ADoorwayToOtherWorldsAnOpenSourceMUD))
                .size(13.0)
                .color(theme.text),
        );
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            primary_button(ui, t(S::Done), theme).clicked()
        })
        .inner
    });
    if response.inner || response.should_close() {
        DialogResult::Closed
    } else {
        DialogResult::Open
    }
}

fn link(ctx: &egui::Context, url: &str, theme: &Theme) -> DialogResult {
    let response = modal(ctx, "link-confirm", 460.0, theme, |ui| {
        ui.label(RichText::new(t(S::LinkOpenQuestion)).size(20.0).color(theme.text));
        ui.add_space(8.0);
        ui.label(RichText::new(url).size(13.0).monospace().color(theme.muted));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let open = primary_button(ui, t(S::LinkOpen), theme).clicked();
            let cancel = secondary_button(ui, t(S::Cancel2)).clicked();
            (open, cancel)
        })
        .inner
    });
    match response.inner {
        (true, _) => DialogResult::OpenLink(url.to_string()),
        (_, true) => DialogResult::Closed,
        _ if response.should_close() => DialogResult::Closed,
        _ => DialogResult::Open,
    }
}

/// The C# `MudletChooser`: the title, what an import does, where a package goes, and Cancel,
/// Choose a profile or package file..., Choose a profile folder... (520 wide, 28 padding).
fn mudlet_chooser(ctx: &egui::Context, target: Option<&str>, theme: &Theme) -> DialogResult {
    let response = modal(ctx, "mudlet-import", 464.0, theme, |ui| {
        ui.label(RichText::new(t(S::MudletImportTitle)).size(24.0).color(theme.text));
        ui.add_space(12.0);
        ui.add(egui::Label::new(RichText::new(t(S::MudletImportIntro)).size(13.0).color(theme.muted)).wrap());
        ui.add_space(12.0);
        let where_to = match target {
            Some(name) => tf(S::MudletImportPackageTarget, &[&name]),
            None => t(S::MudletImportPackageNoTarget).to_string(),
        };
        ui.add(egui::Label::new(RichText::new(where_to).size(12.0).color(theme.muted)).wrap());
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let cancel = ui
                .add(
                    egui::Button::new(RichText::new(format!("\u{00d7}  {}", t(S::Cancel2))).size(14.0))
                        .min_size(egui::vec2(60.0, 32.0)),
                )
                .clicked();
            let file = secondary_button(ui, t(S::MudletImportChooseFile)).clicked();
            let folder = primary_button(ui, t(S::MudletImportChooseFolder), theme).clicked();
            (cancel, file, folder)
        })
        .inner
    });
    match response.inner {
        (_, _, true) => DialogResult::MudletPick(MudletPick::Folder),
        (_, true, _) => DialogResult::MudletPick(MudletPick::File),
        (true, _, _) => DialogResult::Closed,
        _ if response.should_close() => DialogResult::Closed,
        _ => DialogResult::Open,
    }
}

/// The C# `MudletSummaryWindow`: Import finished, the summary in a scroll area, Done.
fn mudlet_summary(ctx: &egui::Context, text: &str, theme: &Theme) -> DialogResult {
    let response = modal(ctx, "mudlet-import-summary", 564.0, theme, |ui| {
        ui.label(RichText::new(t(S::MudletImportFinished)).size(24.0).color(theme.text));
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .id_salt("mudlet-summary-text")
            .max_height(372.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(RichText::new(text).size(13.0).color(theme.text))
                        .wrap()
                        .selectable(true),
                );
            });
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            primary_button(ui, t(S::Done), theme).clicked()
        })
        .inner
    });
    if response.inner || response.should_close() {
        DialogResult::Closed
    } else {
        DialogResult::Open
    }
}
