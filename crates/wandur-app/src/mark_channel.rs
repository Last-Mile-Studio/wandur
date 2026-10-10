//! The "Mark as channel" dialog (C# `MarkChannelDialog` over `MarkChannelViewModel`): one
//! transcript line, the shape Wandur guessed for it, and a live preview of what the rule would
//! catch over the last 200 transcript lines, so over- and under-matching show before anything
//! is saved. Editing a piece (head, speaker, separator) recomposes the rule; editing the rule
//! directly leaves the pieces as they were. Save rule teaches the world the rule; Not a channel
//! teaches it an exclusion for the same shape instead.
//!
//! [`MarkChannel`] is the model (no UI, tested directly); [`MarkChannel::show`] draws it.

use std::sync::Arc;

use egui::{Align, Layout, RichText, Ui};
use wandur_core::channels::proposer::{self, Proposal};
use wandur_core::channels::rules::{self, ChannelRule, RuleSet};
use wandur_core::l10n::{S, t, tf};

use crate::session_tab::SessionId;
use crate::theme::Theme;

/// Transcript lines the preview looks at.
pub const PREVIEW_LINES: usize = 200;
/// Matches the preview lists.
pub const PREVIEW_SHOWN: usize = 5;

/// One line of the preview: what the rule made of a transcript line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewMatch {
    pub speaker: String,
    pub text: String,
}

impl PreviewMatch {
    pub fn label(&self) -> String {
        if self.speaker.is_empty() {
            self.text.clone()
        } else {
            format!("{}: {}", self.speaker, self.text)
        }
    }
}

/// What the dialog asked for when it closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkResult {
    Open,
    Cancelled,
    /// Save rule, or Not a channel (an exclusion): teach the session's world this rule.
    Teach(SessionId, ChannelRule),
}

pub struct MarkChannel {
    pub session: SessionId,
    /// The example line, plain and trimmed.
    pub example: String,
    lines: Vec<String>,
    proposal: Proposal,
    /// The rules in force for the session (the channels it knows, their replies).
    known: Arc<RuleSet>,
    /// The channel this line is shown under today, if any.
    pub current_channel: Option<String>,
    /// The session's world can be taught (it is a saved world, not the demo).
    pub can_teach: bool,
    /// The known channels, then "New channel...".
    pub choices: Vec<String>,
    pub head_pattern: String,
    pub speaker_pattern: String,
    pub separator_pattern: String,
    pub pattern: String,
    pub channel_index: usize,
    pub new_channel_name: String,
    pub reply_command: String,
    pub private: bool,
    pub match_count: usize,
    pub matches: Vec<PreviewMatch>,
    pub preview_summary: String,
    pub error: String,
}

impl MarkChannel {
    /// The dialog for `example` (and a narrowing `second`), over the session's recent `lines`
    /// and the rules it runs.
    pub fn new(
        session: SessionId,
        example: &str,
        second: Option<&str>,
        lines: Vec<String>,
        known: Arc<RuleSet>,
        can_teach: bool,
    ) -> Self {
        let example = rules::strip(example).trim().to_string();
        let proposal = proposer::propose(&example, second);
        let current_channel = known.match_plain(&example).map(|m| m.channel);
        let mut choices = known.channels();
        let guess = proposal.rule.channel.clone();
        let known_index = choices.iter().position(|c| c.eq_ignore_ascii_case(&guess));
        choices.push(t(S::TeachChannelNewChannel).into());
        let mut model = Self {
            session,
            lines,
            current_channel,
            can_teach,
            channel_index: known_index.unwrap_or(choices.len() - 1),
            new_channel_name: if known_index.is_some() {
                String::new()
            } else {
                guess.clone()
            },
            choices,
            head_pattern: proposal.head_pattern.clone(),
            speaker_pattern: proposal.speaker_pattern.clone(),
            separator_pattern: proposal.separator_pattern.clone(),
            pattern: proposal.rule.pattern.clone(),
            reply_command: proposal.rule.reply_command.clone().unwrap_or_default(),
            private: proposal.rule.private,
            proposal,
            known,
            example,
            match_count: 0,
            matches: Vec::new(),
            preview_summary: String::new(),
            error: String::new(),
        };
        if known_index.is_some() {
            model.adopt_channel(&guess);
        }
        model.refresh_preview();
        model
    }

    pub fn proposal(&self) -> &Proposal {
        &self.proposal
    }

    /// The transcript lines the preview runs over.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn is_new_channel(&self) -> bool {
        self.channel_index + 1 == self.choices.len()
    }

    /// The channel as the fields stand, trimmed and lower case.
    pub fn channel(&self) -> String {
        let name = if self.is_new_channel() {
            self.new_channel_name.as_str()
        } else {
            self.choices.get(self.channel_index).map_or("", String::as_str)
        };
        name.trim().to_lowercase()
    }

    /// The rule as the fields stand, or `None` while they do not make one.
    pub fn rule(&self) -> Option<ChannelRule> {
        let channel = self.channel();
        let ok = !channel.is_empty()
            && channel.chars().count() <= rules::MAX_CHANNEL
            && !self.pattern.is_empty()
            && self.pattern.chars().count() <= rules::MAX_PATTERN
            && rules::compile(&self.pattern).is_some();
        let reply = self.reply_command.trim();
        ok.then(|| ChannelRule {
            channel,
            pattern: self.pattern.clone(),
            reply_command: (!reply.is_empty()).then(|| reply.to_string()),
            private: self.private,
            ..ChannelRule::default()
        })
    }

    pub fn can_save(&self) -> bool {
        self.can_teach && self.rule().is_some()
    }

    pub fn can_exclude(&self) -> bool {
        self.current_channel.is_some() && self.can_teach
    }

    pub fn exclude_help(&self) -> String {
        self.current_channel
            .as_ref()
            .map(|c| tf(S::TeachChannelNotAChannelHelp, &[c]))
            .unwrap_or_default()
    }

    pub fn preview_title(&self) -> String {
        tf(S::TeachChannelPreview, &[&self.lines.len()])
    }

    pub fn set_head_pattern(&mut self, value: &str) {
        self.head_pattern = value.into();
        self.recompose();
    }

    pub fn set_speaker_pattern(&mut self, value: &str) {
        self.speaker_pattern = value.into();
        self.recompose();
    }

    pub fn set_separator_pattern(&mut self, value: &str) {
        self.separator_pattern = value.into();
        self.recompose();
    }

    /// The rule edited directly: the pieces stay as they are.
    pub fn set_pattern(&mut self, value: &str) {
        self.pattern = value.into();
        self.refresh_preview();
    }

    pub fn set_channel_index(&mut self, index: usize) {
        self.channel_index = index.min(self.choices.len() - 1);
        if !self.is_new_channel() {
            let channel = self.channel();
            self.adopt_channel(&channel);
        }
        self.refresh_preview();
    }

    pub fn set_new_channel_name(&mut self, value: &str) {
        self.new_channel_name = value.into();
        self.refresh_preview();
    }

    pub fn set_reply_command(&mut self, value: &str) {
        self.reply_command = value.into();
        self.refresh_preview();
    }

    pub fn set_private(&mut self, value: bool) {
        self.private = value;
        self.refresh_preview();
    }

    /// A known channel brings its reply command and privacy with it.
    fn adopt_channel(&mut self, channel: &str) {
        let existing = self.known.for_channel(channel);
        self.reply_command = existing
            .and_then(|r| r.reply_command.clone())
            .or_else(|| proposer::guess_reply(channel))
            .unwrap_or_default();
        self.private = existing.map_or_else(|| proposer::is_private_channel(channel), |r| r.private);
    }

    fn recompose(&mut self) {
        self.pattern = proposer::compose(
            &self.head_pattern,
            &self.speaker_pattern,
            &self.separator_pattern,
            &self.proposal.closing,
        );
        self.refresh_preview();
    }

    /// Runs the rule over the recent transcript.
    fn refresh_preview(&mut self) {
        self.matches.clear();
        let compiled = rules::compile(&self.pattern);
        self.error = if compiled.is_none() {
            t(S::TeachChannelInvalidPattern).into()
        } else if self.is_new_channel() && self.channel().is_empty() {
            t(S::TeachChannelNeedsName).into()
        } else if !self.can_teach {
            t(S::TeachChannelNoProfile).into()
        } else {
            String::new()
        };
        let mut count = 0;
        if let Some(expression) = &compiled {
            let rule = self
                .rule()
                .unwrap_or_else(|| ChannelRule::new("preview", &self.pattern, None));
            for line in &self.lines {
                let Some(m) = rules::apply_compiled(&rule, expression, line) else {
                    continue;
                };
                count += 1;
                if self.matches.len() < PREVIEW_SHOWN {
                    self.matches.push(PreviewMatch {
                        speaker: m.speaker,
                        text: m.text,
                    });
                }
            }
            // The example itself may have scrolled out of the window the preview looks at.
            if count == 0
                && let Some(own) = rules::apply_compiled(&rule, expression, &self.example)
            {
                count = 1;
                self.matches.push(PreviewMatch {
                    speaker: own.speaker,
                    text: own.text,
                });
            }
        }
        self.match_count = count;
        self.preview_summary = if count == 0 {
            t(S::TeachChannelPreviewNone).into()
        } else {
            tf(S::TeachChannelPreviewCount, &[&count, &self.lines.len().max(1)])
        };
    }

    /// Save rule: the rule to teach, or `None` (the error says why).
    pub fn save(&mut self) -> Option<ChannelRule> {
        if !self.can_teach {
            self.error = t(S::TeachChannelNoProfile).into();
            return None;
        }
        self.rule()
    }

    /// Not a channel: the same shape, taught as something to leave alone.
    pub fn exclude(&mut self) -> Option<ChannelRule> {
        let channel = self.current_channel.clone()?;
        if rules::compile(&self.pattern).is_none() {
            self.error = t(S::TeachChannelInvalidPattern).into();
            return None;
        }
        if !self.can_teach {
            self.error = t(S::TeachChannelNoProfile).into();
            return None;
        }
        Some(ChannelRule::exclusion(&channel, &self.pattern))
    }

    /// The app could not save the rule (validation, the settings file): keep the dialog open.
    pub fn failed(&mut self, error: String) {
        self.error = error;
    }

    /// Draw the dialog, centred over the dimmed window as the C# one is over its owner.
    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme) -> MarkResult {
        let screen = ctx.content_rect();
        let width = (screen.width() - 40.0).clamp(480.0, 680.0);
        let height = (screen.height() - 40.0).clamp(440.0, 640.0);
        let mut result = MarkResult::Open;
        let modal = egui::Modal::new(egui::Id::new(("mark-channel", self.session)))
            .backdrop_color(theme.dim())
            .frame(
                egui::Frame::new()
                    .fill(theme.panel)
                    .stroke(egui::Stroke::new(1.0, theme.border))
                    .corner_radius(4),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.set_height(height);
                ui.spacing_mut().interact_size.y = 30.0;
                let full = ui.max_rect();
                let footer_h =
                    58.0 + if self.error.is_empty() { 0.0 } else { 22.0 } + if self.can_exclude() { 34.0 } else { 0.0 };
                let foot = egui::Rect::from_min_max(egui::pos2(full.left(), full.bottom() - footer_h), full.max);
                let body = egui::Rect::from_min_max(full.min, egui::pos2(full.right(), foot.top()));
                ui.painter()
                    .hline(full.x_range(), foot.top(), egui::Stroke::new(1.0, theme.border));
                ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(24, 20))
                            .show(ui, |ui| self.body(ui, theme, width - 48.0));
                    });
                });
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(foot.shrink2(egui::vec2(24.0, 12.0))),
                    |ui| self.footer(ui, theme, &mut result),
                );
            });
        if result == MarkResult::Open && (modal.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
            result = MarkResult::Cancelled;
        }
        result
    }

    fn body(&mut self, ui: &mut Ui, theme: &Theme, width: f32) {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 6.0;
        let mono = |size: f32| egui::FontId::monospace(size);
        eyebrow(ui, t(S::TeachChannelExample), theme);
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, theme.border))
            .corner_radius(4)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(width - 22.0);
                ui.add(egui::Label::new(RichText::new(&self.example).font(mono(12.0)).color(theme.text)).wrap());
            });
        ui.add_space(8.0);
        ui.add(egui::Label::new(RichText::new(t(S::TeachChannelHelp)).size(12.0).color(theme.muted)).wrap());
        ui.add_space(8.0);

        // The three pieces.
        let gap = 10.0;
        let third = (width - 2.0 * gap) / 3.0;
        let mut head = self.head_pattern.clone();
        let mut speaker = self.speaker_pattern.clone();
        let mut separator = self.separator_pattern.clone();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            piece(ui, t(S::TeachChannelHead), &mut head, "teach-head", third, theme);
            piece(
                ui,
                t(S::TeachChannelSpeaker),
                &mut speaker,
                "teach-speaker",
                third,
                theme,
            );
            piece(
                ui,
                t(S::TeachChannelSeparator),
                &mut separator,
                "teach-separator",
                third,
                theme,
            );
        });
        if head != self.head_pattern {
            self.set_head_pattern(&head);
        }
        if speaker != self.speaker_pattern {
            self.set_speaker_pattern(&speaker);
        }
        if separator != self.separator_pattern {
            self.set_separator_pattern(&separator);
        }
        let read: Vec<String> = [&self.proposal.head, &self.proposal.speaker, &self.proposal.separator]
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| format!("\u{201c}{s}\u{201d}"))
            .collect();
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(read.join("  \u{b7}  "))
                    .font(mono(11.0))
                    .color(theme.muted),
            )
            .wrap(),
        );
        ui.add_space(8.0);

        field_label(ui, t(S::TeachChannelPattern), theme);
        let mut pattern = self.pattern.clone();
        crate::a11y::named(
            ui.add(
                egui::TextEdit::singleline(&mut pattern)
                    .id_salt("teach-pattern")
                    .font(egui::TextStyle::Monospace)
                    .char_limit(rules::MAX_PATTERN)
                    .margin(egui::Margin::symmetric(10, 7))
                    .desired_width(width),
            ),
            t(S::TeachChannelPattern),
        );
        if pattern != self.pattern {
            self.set_pattern(&pattern);
        }
        ui.add_space(8.0);

        // Channel and reply command.
        let half = (width - gap) / 2.0;
        let mut index = self.channel_index;
        let mut name = self.new_channel_name.clone();
        let mut reply = self.reply_command.clone();
        let mut private = self.private;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.vertical(|ui| {
                ui.set_width(half);
                field_label(ui, t(S::TeachChannelChannel), theme);
                crate::a11y::named_combo(
                    egui::ComboBox::from_id_salt("teach-channel")
                        .selected_text(RichText::new(self.choices.get(index).map_or("", String::as_str)).size(12.0))
                        .width(half)
                        .show_ui(ui, |ui| {
                            for (i, choice) in self.choices.iter().enumerate() {
                                ui.selectable_value(&mut index, i, choice);
                            }
                        }),
                    t(S::TeachChannelChannel),
                );
                if self.is_new_channel() {
                    ui.add_space(4.0);
                    crate::a11y::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut name)
                                .id_salt("teach-channel-name")
                                .hint_text(t(S::TeachChannelNewChannelName))
                                .char_limit(rules::MAX_CHANNEL)
                                .margin(egui::Margin::symmetric(10, 7))
                                .desired_width(half),
                        ),
                        t(S::TeachChannelNewChannelName),
                    );
                }
            });
            ui.vertical(|ui| {
                ui.set_width(half);
                field_label(ui, t(S::TeachChannelReply), theme);
                crate::a11y::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut reply)
                            .id_salt("teach-reply")
                            .char_limit(rules::MAX_REPLY)
                            .margin(egui::Margin::symmetric(10, 7))
                            .desired_width(half),
                    ),
                    t(S::TeachChannelReply),
                );
                ui.add_space(4.0);
                ui.checkbox(&mut private, RichText::new(t(S::TeachChannelPrivate)).size(12.0));
            });
        });
        if index != self.channel_index {
            self.set_channel_index(index);
        }
        if name != self.new_channel_name {
            self.set_new_channel_name(&name);
        }
        if reply != self.reply_command {
            self.set_reply_command(&reply);
        }
        if private != self.private {
            self.set_private(private);
        }
        ui.add_space(10.0);

        // The preview.
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, theme.border))
            .corner_radius(4)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(width - 26.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                eyebrow(ui, &self.preview_title(), theme);
                ui.add(
                    egui::Label::new(
                        RichText::new(&self.preview_summary)
                            .size(12.0)
                            .strong()
                            .color(theme.text),
                    )
                    .wrap(),
                );
                for m in &self.matches {
                    ui.add(egui::Label::new(RichText::new(m.label()).font(mono(12.0)).color(theme.text)).wrap());
                }
            });
    }

    fn footer(&mut self, ui: &mut Ui, theme: &Theme, result: &mut MarkResult) {
        ui.spacing_mut().item_spacing.y = 6.0;
        if !self.error.is_empty() {
            ui.label(RichText::new(&self.error).size(12.0).color(theme.error));
        }
        if self.can_exclude() {
            ui.add(egui::Label::new(RichText::new(self.exclude_help()).size(11.0).color(theme.muted)).wrap());
        }
        ui.horizontal(|ui| {
            if self.can_exclude()
                && crate::dialogs::secondary_button(ui, t(S::TeachChannelNotAChannel)).clicked()
                && let Some(rule) = self.exclude()
            {
                *result = MarkResult::Teach(self.session, rule);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let save = ui.add_enabled_ui(self.can_save(), |ui| {
                    crate::dialogs::primary_button(ui, t(S::TeachChannelSave), theme)
                });
                if save.inner.clicked()
                    && let Some(rule) = self.save()
                {
                    *result = MarkResult::Teach(self.session, rule);
                }
                if crate::dialogs::secondary_button(ui, t(S::Cancel)).clicked() {
                    *result = MarkResult::Cancelled;
                }
            });
        });
    }
}

/// A small bold caption over a group (the C# "eyebrow" text).
fn eyebrow(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(RichText::new(text).size(10.0).strong().color(theme.muted));
}

fn field_label(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(theme.muted)).wrap());
}

/// One piece of the shape: its label over a monospace field.
fn piece(ui: &mut Ui, label: &str, value: &mut String, id: &str, width: f32, theme: &Theme) {
    ui.vertical(|ui| {
        ui.set_width(width);
        field_label(ui, label, theme);
        crate::a11y::named(
            ui.add(
                egui::TextEdit::singleline(value)
                    .id_salt(id)
                    .font(egui::TextStyle::Monospace)
                    .char_limit(rules::MAX_PATTERN)
                    .margin(egui::Margin::symmetric(10, 7))
                    .desired_width(width),
            ),
            label,
        );
    });
}

#[cfg(test)]
mod tests {
    //! The model cases of the C# `TeachChannelTests`.
    use super::*;
    use wandur_core::channels::families;
    use wandur_core::l10n::{Language, override_thread};

    fn transcript() -> Vec<String> {
        [
            "You are standing in a wide green field.",
            "[CLAN] Vex: meeting at dawn",
            "[CLAN] Talon: bring the ship",
            "A small bird lands nearby.",
            "[CLAN] Vex: and the crew",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn the_dialog_proposes_a_rule_with_a_preview_and_follows_edits() {
        override_thread(Some(Language::En));
        let smaug = families::for_world(&[], Some("SMAUG 1.4a"));
        let mut m = MarkChannel::new(1, "[CLAN] Vex: meeting at dawn", None, transcript(), smaug, true);
        assert_eq!(m.example, "[CLAN] Vex: meeting at dawn");
        assert_eq!(m.head_pattern, r"\[CLAN\] ");
        assert_eq!(m.speaker_pattern, "@?[A-Za-z]+");
        assert_eq!(m.separator_pattern, ": ");
        assert_eq!(m.pattern, r"^\[CLAN\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$");
        // SMAUG knows no clan channel, so the picker lands on a new channel named by the head.
        assert!(m.is_new_channel());
        assert_eq!(m.new_channel_name, "clan");
        assert_eq!(m.channel(), "clan");
        assert_eq!(m.reply_command, "clan");
        assert!(m.choices.contains(&"ooc".to_string()));
        assert_eq!(m.choices.last().map(String::as_str), Some(t(S::TeachChannelNewChannel)));
        assert_eq!(m.current_channel, None);
        assert!(!m.can_exclude());
        assert_eq!(m.match_count, 3);
        let labels: Vec<String> = m.matches.iter().map(PreviewMatch::label).collect();
        assert_eq!(
            labels,
            ["Vex: meeting at dawn", "Talon: bring the ship", "Vex: and the crew"]
        );
        assert!(m.preview_summary.contains('3'));
        assert!(m.can_save());

        // Editing a piece recomposes the rule and the preview follows; a head that fits nothing
        // says so.
        m.set_head_pattern(r"\[SHIP\] ");
        assert_eq!(m.pattern, r"^\[SHIP\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$");
        assert_eq!(m.match_count, 0);
        assert_eq!(m.preview_summary, t(S::TeachChannelPreviewNone));
        m.set_head_pattern(r"\[CLAN\] ");
        assert_eq!(m.match_count, 3);
        // The rule can be edited directly; the pieces are left alone and a broken one cannot be
        // saved.
        m.set_pattern(r"^\[CLAN\] (?<speaker>[A-Za-z");
        assert!(!m.can_save());
        assert_eq!(m.error, t(S::TeachChannelInvalidPattern));
        m.set_pattern(r"^\[CLAN\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$");
        assert!(m.can_save());
        assert_eq!(m.head_pattern, r"\[CLAN\] ");
        assert_eq!(
            m.save(),
            Some(ChannelRule::new(
                "clan",
                r"^\[CLAN\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$",
                Some("clan")
            ))
        );
        // A new channel needs a name.
        m.set_new_channel_name("  ");
        assert_eq!(m.error, t(S::TeachChannelNeedsName));
        assert!(!m.can_save());
        override_thread(None);
    }

    #[test]
    fn a_line_the_family_shows_can_be_taught_away() {
        override_thread(Some(Language::En));
        let smaug = families::for_world(&[], Some("SMAUG 1.4a"));
        let mut m = MarkChannel::new(
            1,
            "[OOC] Aldric: anyone selling a lantern?",
            None,
            vec!["[OOC] Aldric: anyone selling a lantern?".into()],
            smaug,
            true,
        );
        assert_eq!(m.current_channel.as_deref(), Some("ooc"));
        assert!(m.can_exclude());
        assert!(!m.is_new_channel());
        assert_eq!(m.channel(), "ooc");
        assert_eq!(m.reply_command, "ooc", "the known channel brings its reply");
        assert!(m.exclude_help().contains("ooc"));
        let rule = m.exclude().unwrap();
        assert!(rule.exclude);
        assert_eq!(rule.channel, "ooc");
        assert_eq!(rule.pattern, m.pattern);
        override_thread(None);
    }

    #[test]
    fn the_demo_cannot_be_taught() {
        override_thread(Some(Language::En));
        let mut m = MarkChannel::new(
            1,
            "[CLAN] Vex: meeting at dawn",
            None,
            Vec::new(),
            families::family(None),
            false,
        );
        assert!(!m.can_save());
        assert_eq!(m.error, t(S::TeachChannelNoProfile));
        assert_eq!(m.save(), None);
        // The example itself counts when the transcript window holds nothing.
        assert_eq!(m.match_count, 1);
        override_thread(None);
    }

    #[test]
    fn a_selection_gives_the_example_and_the_narrowing_line() {
        use crate::terminal_view::MenuTarget;
        let target = MenuTarget {
            line: "under the pointer".into(),
            selection: Some("\n[OOC] Aldric: hello\n(OOC) Bix: hi\nmore".into()),
        };
        assert_eq!(
            target.examples(),
            ("[OOC] Aldric: hello".into(), Some("(OOC) Bix: hi".into()))
        );
        let plain = MenuTarget {
            line: "[CLAN] Vex: x   ".into(),
            selection: None,
        };
        assert_eq!(plain.examples(), ("[CLAN] Vex: x".into(), None));
    }
}
