// SPDX-License-Identifier: GPL-3.0-only

//! The furniture around the document: the toolbar, the find bar, the status
//! bar, and the panes that open in the context drawer.
//!
//! # Why the toolbar asks the engine whether a button applies
//!
//! Every button here is the same command the keyboard runs, and a command
//! answers "would this apply here?" by returning a transaction or not. So a
//! button is disabled exactly when the shortcut would do nothing — bold greys
//! out inside a code block because the schema says a code block admits no
//! marks, and nobody had to write that rule down twice.

use cosmic::iced::{Alignment, Length};
use cosmic::prelude::*;
use cosmic::widget::{self, menu};

use nib_model::search::{self, Matching};
use nib_model::{basic, commands};

use crate::app::{App, Find, Message};
use crate::config::{Appearance, CaretShape, MAXIMUM_TEXT_SIZE, MINIMUM_TEXT_SIZE};
use crate::fl;

impl App {
    /// The formatting toolbar.
    pub(crate) fn toolbar(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        // A vertical rule is `Fill`-height by default, which makes the row
        // that holds it `Fill` too — and a `Fill` row in a column with a
        // `Fill` page underneath it splits the window in half. Pinning the
        // rule to the height of a button is what keeps the toolbar a toolbar.
        let separator = || {
            widget::divider::vertical::default().height(Length::Fixed(20.0))
        };

        let mark = |icon: &'static str, tooltip: String, name: &'static str| {
            let enabled = nib::toolbar_command(self.state(), name).is_some();
            let active = self.mark_is_on(name);
            widget::tooltip(
                widget::button::icon(widget::icon::from_name(icon).size(16))
                    .class(if active {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Icon
                    })
                    .on_press_maybe(enabled.then_some(Message::Command(name))),
                widget::text::body(tooltip),
                widget::tooltip::Position::Bottom,
            )
        };

        let block = |label: String, name: &'static str, level: i64| {
            let enabled = self
                .schema()
                .node_id(name)
                .map(|typ| {
                    let attrs = (level > 0).then(|| nib_model::attrs! { "level" => level });
                    commands::set_block_type(typ, attrs)
                })
                .is_some_and(|command| command(self.state()).is_some());
            widget::button::text(label)
                .class(cosmic::theme::Button::Text)
                .on_press_maybe(enabled.then_some(Message::Block(name, level)))
        };

        let structure = |icon: &'static str, tooltip: String, name: &'static str| {
            widget::tooltip(
                widget::button::icon(widget::icon::from_name(icon).size(16))
                    .on_press_maybe(
                        nib::toolbar_command(self.state(), name)
                            .is_some()
                            .then_some(Message::Command(name)),
                    ),
                widget::text::body(tooltip),
                widget::tooltip::Position::Bottom,
            )
        };

        widget::row::with_children(vec![
            block(fl!("paragraph"), basic::nodes::PARAGRAPH, 0).into(),
            block("H1".into(), basic::nodes::HEADING, 1).into(),
            block("H2".into(), basic::nodes::HEADING, 2).into(),
            block("H3".into(), basic::nodes::HEADING, 3).into(),
            separator().into(),
            mark("format-text-bold-symbolic", fl!("bold"), "bold").into(),
            mark("format-text-italic-symbolic", fl!("italic"), "italic").into(),
            mark(
                "format-text-underline-symbolic",
                fl!("underline"),
                "underline",
            )
            .into(),
            mark(
                "format-text-strikethrough-symbolic",
                fl!("strikethrough"),
                "strikethrough",
            )
            .into(),
            mark("format-text-code-symbolic", fl!("code"), "code").into(),
            separator().into(),
            structure(
                "format-unordered-list-symbolic",
                fl!("bullet-list"),
                "bullet_list",
            )
            .into(),
            structure(
                "format-ordered-list-symbolic",
                fl!("ordered-list"),
                "ordered_list",
            )
            .into(),
            structure("format-indent-more-symbolic", fl!("quote"), "quote").into(),
            widget::space::horizontal().into(),
            structure("edit-undo-symbolic", fl!("undo"), "undo").into(),
            structure("edit-redo-symbolic", fl!("redo"), "redo").into(),
        ])
        .align_y(Alignment::Center)
        .spacing(spacing.space_xxxs)
        .padding(spacing.space_xxs)
        .into()
    }

    /// What a right-click offers.
    ///
    /// The same commands as the toolbar and the keyboard, and disabled by the
    /// same question: a command that would do nothing here is one the menu
    /// shows greyed rather than one that does nothing when pressed.
    pub(crate) fn context_menu(&self) -> Vec<menu::Tree<Message>> {
        let item = |label: String, message: Option<Message>| {
            menu::Tree::new(cosmic::Element::from(
                widget::button::text(label)
                    .class(cosmic::theme::Button::MenuItem)
                    .width(Length::Fill)
                    .on_press_maybe(message),
            ))
        };
        let command = |label: String, name: &'static str| {
            item(
                label,
                nib::toolbar_command(self.state(), name)
                    .is_some()
                    .then_some(Message::Command(name)),
            )
        };
        let has_selection = !self.state().selection().is_empty();

        // A misspelling under the caret puts its corrections at the top, where
        // a right-click on a squiggle expects to find them.
        let mut items = Vec::new();
        if let Some(speller) = self.speller()
            && let Some((word, from, to)) =
                speller.word_at(self.state().doc(), self.state().selection().head())
        {
            for suggestion in speller.suggest(&word).into_iter().take(5) {
                items.push(item(
                    suggestion.clone(),
                    Some(Message::Correct(from, to, suggestion)),
                ));
            }
            items.push(item(
                fl!("add-to-dictionary"),
                Some(Message::Learn(word)),
            ));
        }

        items.extend([
            item(fl!("cut"), has_selection.then_some(Message::Clipboard("cut"))),
            item(fl!("copy"), has_selection.then_some(Message::Clipboard("copy"))),
            item(fl!("paste"), Some(Message::Clipboard("paste"))),
            item(fl!("select-all"), Some(Message::Command("select_all"))),
            command(fl!("bold"), "bold"),
            command(fl!("italic"), "italic"),
            command(fl!("code"), "code"),
            command(fl!("quote"), "quote"),
            command(fl!("bullet-list"), "bullet_list"),
        ]);
        items
    }

    /// The find and replace bar.
    pub(crate) fn find_bar(find: &Find, project: bool) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let count = if find.query.is_empty() {
            String::new()
        } else if find.hits.is_empty() {
            fl!("no-matches")
        } else {
            let at = find.current.map_or(0, |i| i + 1);
            fl!("match-count", at = at, total = find.hits.len())
        };

        let matching = match find.matching {
            Matching::Loose => fl!("matching-loose"),
            Matching::CaseSensitive => fl!("matching-case"),
            Matching::WholeWord => fl!("matching-word"),
            Matching::Regex => fl!("matching-regex"),
        };

        let mut row = widget::row::with_capacity(9)
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs)
            .padding(spacing.space_xxs)
            .push(
                widget::text_input(fl!("find"), &find.query)
                    .id(find_input())
                    .on_input(Message::FindChanged)
                    .on_submit(|_| Message::FindNext)
                    .width(Length::FillPortion(2)),
            )
            .push(
                widget::button::text(matching)
                    .class(cosmic::theme::Button::Text)
                    .on_press(Message::CycleMatching),
            )
            .push(
                widget::button::icon(widget::icon::from_name("go-up-symbolic").size(16))
                    .on_press(Message::FindPrevious),
            )
            .push(
                widget::button::icon(widget::icon::from_name("go-down-symbolic").size(16))
                    .on_press(Message::FindNext),
            )
            .push(
                widget::tooltip(
                    widget::button::icon(
                        widget::icon::from_name("folder-saved-search-symbolic").size(16),
                    )
                    // Greyed rather than hidden: a button that appears when a
                    // folder opens is a button nobody knows is there.
                    .on_press_maybe(project.then_some(Message::FindInProject)),
                    widget::text::caption(fl!("find-in-project")),
                    widget::tooltip::Position::Bottom,
                ),
            )
            .push(widget::text::caption(count).width(Length::Shrink))
            .push(
                widget::text_input(fl!("replace-with"), &find.replacement)
                    .on_input(Message::ReplaceChanged)
                    .width(Length::FillPortion(2)),
            )
            .push(widget::button::text(fl!("replace")).on_press(Message::ReplaceOne))
            .push(widget::button::text(fl!("replace-all")).on_press(Message::ReplaceAll))
            .push(
                widget::button::icon(widget::icon::from_name("window-close-symbolic").size(16))
                    .on_press(Message::ToggleFind),
            );

        if let Some(error) = &find.error {
            row = row.push(widget::text::caption(error.clone()));
        }
        widget::container(row).into()
    }

    /// The Vim mode, and any half-typed command, for the status bar.
    ///
    /// Only when modal editing is on: a status bar that says NORMAL to someone
    /// who never asked for modes is a status bar telling them something is
    /// wrong.
    fn vim_mode(&self) -> Option<Element<'_, Message>> {
        if !self.vim_enabled() {
            return None;
        }
        Some(widget::text::caption(format!("-- {} --", self.mode_label())).into())
    }

    /// The bar an error appears in.
    ///
    /// A bar rather than a dialog: a failed save is something to read and act
    /// on, not something to dismiss before the document can be touched again.
    pub(crate) fn error_bar(error: &str) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        widget::container(
            widget::row::with_capacity(3)
                .align_y(Alignment::Center)
                .spacing(spacing.space_xxs)
                .push(widget::icon::from_name("dialog-error-symbolic").size(16))
                .push(widget::text::body(error.to_owned()).width(Length::Fill))
                .push(
                    widget::button::icon(
                        widget::icon::from_name("window-close-symbolic").size(16),
                    )
                    .on_press(Message::DismissError),
                ),
        )
        .class(cosmic::theme::Container::Primary)
        .padding(spacing.space_xxs)
        .into()
    }

    /// The status bar: what this document is, and how much of it there is.
    pub(crate) fn status_bar(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let count = search::count(self.state().doc());
        let name = crate::document::title(self.path());
        let modified = if self.is_dirty() { "\u{2022} " } else { "" };

        widget::container(
            widget::row::with_capacity(5)
                .align_y(Alignment::Center)
                .spacing(spacing.space_s)
                .push(widget::text::caption(format!("{modified}{name}")))
                .push_maybe(self.vim_mode())
                .push(widget::text::caption(if self.format().lossless() {
                    self.format().to_string()
                } else {
                    // A format that cannot hold everything the schema does is
                    // worth a mark, once, where the format is already named —
                    // rather than a dialog on every save that nobody reads.
                    format!("{} •", self.format())
                }))
                .push(widget::space::horizontal())
                .push(widget::text::caption(fl!(
                    "word-count",
                    words = count.words,
                    characters = count.characters
                )))
                .push(widget::text::caption(fl!(
                    "block-count",
                    blocks = count.blocks
                ))),
        )
        .padding(spacing.space_xxs)
        .into()
    }

    /// The settings pane.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn settings_page(&self) -> Element<'_, Message> {
        let config = self.config();
        let spacing = cosmic::theme::spacing();

        let caret = widget::dropdown(
            Self::caret_labels(),
            CaretShape::all()
                .iter()
                .position(|s| *s == config.caret_shape()),
            |index| {
                Message::SetCaret(
                    CaretShape::all()
                        .get(index)
                        .copied()
                        .unwrap_or_default(),
                )
            },
        );

        let appearance = widget::dropdown(
            Self::appearance_labels(),
            Appearance::all()
                .iter()
                .position(|a| *a == config.appearance()),
            |index| {
                Message::SetTheme(
                    Appearance::all().get(index).copied().unwrap_or_default(),
                )
            },
        );

        widget::settings::view_column(vec![
            widget::settings::section()
                .title(fl!("appearance"))
                .add(widget::settings::item(fl!("appearance"), appearance))
                .into(),
            widget::settings::section()
                .title(fl!("text"))
                .add(widget::settings::item(
                    fl!("text-size"),
                    widget::spin_button(
                        config.text_size.to_string(),
                        fl!("text-size"),
                        config.text_size,
                        1,
                        MINIMUM_TEXT_SIZE,
                        MAXIMUM_TEXT_SIZE,
                        Message::SetTextSize,
                    ),
                ))
                .add(widget::settings::item(
                    fl!("measure"),
                    widget::spin_button(
                        if config.measure == 0 {
                            fl!("measure-full")
                        } else {
                            config.measure.to_string()
                        },
                        fl!("measure"),
                        config.measure,
                        4,
                        0,
                        160,
                        Message::SetMeasure,
                    ),
                ))
                .into(),
            widget::settings::section()
                .title(fl!("spelling"))
                .add(widget::settings::item(
                    fl!("spell-check"),
                    widget::toggler(config.spell_check).on_toggle(Message::SetSpellCheck),
                ))
                .add(self.dictionary_setting())
                .into(),
            widget::settings::section()
                .title(fl!("code"))
                .add(widget::settings::item(
                    fl!("line-numbers"),
                    widget::toggler(config.line_numbers).on_toggle(Message::SetLineNumbers),
                ))
                .add(widget::settings::item(
                    fl!("wrap-code"),
                    widget::toggler(config.wrap_code).on_toggle(Message::SetWrapCode),
                ))
                .into(),
            widget::settings::section()
                .title(fl!("keys"))
                .add(widget::settings::item(
                    fl!("vim-bindings"),
                    widget::toggler(config.vim).on_toggle(Message::SetVim),
                ))
                .into(),
            widget::settings::section()
                .title(fl!("caret"))
                .add(widget::settings::item(fl!("caret-shape"), caret))
                .add(widget::settings::item(
                    fl!("caret-blinks"),
                    widget::toggler(config.caret_blinks).on_toggle(Message::SetCaretBlinks),
                ))
                .add(widget::settings::item(
                    fl!("caret-glides"),
                    widget::toggler(config.caret_glides).on_toggle(Message::SetCaretGlides),
                ))
                .into(),
        ])
        .spacing(spacing.space_m)
        .into()
    }

    /// What is in the document, counted.
    ///
    /// The status bar carries the two numbers a writer watches; this is the
    /// rest, for the moments when the question is about the document rather
    /// than the sentence.
    pub(crate) fn statistics_page(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let count = search::count(self.state().doc());
        let headings = search::outline(self.state().doc()).len();
        let row = |label: String, value: usize| {
            widget::settings::item(label, widget::text::body(value.to_string()))
        };
        widget::settings::view_column(vec![
            widget::settings::section()
                .title(crate::document::title(self.path()))
                .add(row(fl!("words"), count.words))
                .add(row(fl!("characters"), count.characters))
                .add(row(fl!("blocks"), count.blocks))
                .add(row(fl!("headings"), headings))
                .into(),
        ])
        .spacing(spacing.space_m)
        .into()
    }

    /// The files opened lately.
    ///
    /// Checked against the filesystem on the way out rather than pruned on the
    /// way in: a file on a drive that is not mounted today should come back
    /// when it is, not be forgotten because it was away.
    pub(crate) fn recent_page(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let files = self.config().recent_files();
        if files.is_empty() {
            return widget::text::body(fl!("recent-none")).into();
        }
        let mut section = widget::settings::section();
        for path in files {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_owned();
            let parent = path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            section = section.add(
                widget::button::custom(
                    widget::column::with_children(vec![
                        widget::text::body(name).into(),
                        widget::text::caption(parent).into(),
                    ])
                    .spacing(spacing.space_none)
                    .width(Length::Fill),
                )
                .class(cosmic::theme::Button::Text)
                .width(Length::Fill)
                .on_press(Message::OpenRecent(path)),
            );
        }
        widget::settings::view_column(vec![
            section.into(),
            widget::button::text(fl!("clear-recent"))
                .class(cosmic::theme::Button::Destructive)
                .on_press(Message::ClearRecent)
                .into(),
        ])
        .spacing(spacing.space_m)
        .into()
    }

    /// Every binding, read out of the keymap rather than written down twice.
    pub(crate) fn shortcuts_page(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        // The application's own keys are not in the editor's keymap: the
        // keymap makes transactions, and opening a print dialog is not one.
        let mut app = widget::settings::section().title(fl!("application"));
        for (keys, what) in APPLICATION_SHORTCUTS {
            app = app.add(widget::settings::item(
                (*keys).to_owned(),
                widget::text::caption(crate::i18n::translate(what)),
            ));
        }

        let mut section = widget::settings::section().title(fl!("editing"));
        for binding in self.keymap().bindings() {
            section = section.add(widget::settings::item(
                describe_binding(binding),
                widget::text::caption(String::new()),
            ));
        }
        widget::settings::view_column(vec![
            widget::text::body(fl!("shortcuts-note")).into(),
            app.into(),
            section.into(),
        ])
        .spacing(spacing.space_m)
        .into()
    }

    /// The about pane.
    pub(crate) fn about_page() -> Element<'static, Message> {
        let spacing = cosmic::theme::spacing();
        widget::column::with_children(vec![
            widget::text::title3(fl!("pencil")).into(),
            widget::text::body(fl!("about-body")).into(),
            widget::text::caption(fl!("about-engine")).into(),
        ])
        .spacing(spacing.space_xs)
        .into()
    }

    /// Which dictionary, chosen from what is installed.
    ///
    /// A list rather than a text field: a field that silently does nothing
    /// when the tag is wrong is worse than no field.
    fn dictionary_setting(&self) -> widget::Row<'_, Message, cosmic::Theme> {
        let installed = nib_spell::Speller::installed();
        if installed.is_empty() {
            return widget::settings::item(
                fl!("spell-language"),
                widget::text::caption(fl!("spell-none")),
            );
        }
        let chosen = self.config().dictionary();
        let selected = installed.iter().position(|l| *l == chosen);
        let labels = installed.clone();
        widget::settings::item(
            fl!("spell-language"),
            widget::dropdown(labels, selected, move |index| {
                Message::SetSpellLanguage(
                    installed.get(index).cloned().unwrap_or_default(),
                )
            }),
        )
    }

    fn caret_labels() -> Vec<String> {
        CaretShape::all().iter().map(|s| s.label()).collect()
    }

    fn appearance_labels() -> Vec<String> {
        Appearance::all().iter().map(|a| a.label()).collect()
    }
}

/// A binding, in the spelling a user recognises.
fn describe_binding(binding: &nib_model::keymap::Binding) -> String {
    use nib_model::keymap::{Key, Mods};

    let mut parts = Vec::new();
    if binding.mods.contains(Mods::PRIMARY) {
        parts.push("Ctrl".to_owned());
    }
    if binding.mods.contains(Mods::ALT) {
        parts.push("Alt".to_owned());
    }
    if binding.mods.contains(Mods::SHIFT) {
        parts.push("Shift".to_owned());
    }
    parts.push(match &binding.key {
        Key::Char(c) => c.to_uppercase().to_string(),
        Key::Enter => "Enter".to_owned(),
        Key::Tab => "Tab".to_owned(),
        Key::Backspace => "Backspace".to_owned(),
        Key::Delete => "Delete".to_owned(),
        Key::Escape => "Escape".to_owned(),
        Key::Home => "Home".to_owned(),
        Key::End => "End".to_owned(),
        Key::PageUp => "Page Up".to_owned(),
        Key::PageDown => "Page Down".to_owned(),
        Key::ArrowLeft => "Left".to_owned(),
        Key::ArrowRight => "Right".to_owned(),
        Key::ArrowUp => "Up".to_owned(),
        Key::ArrowDown => "Down".to_owned(),
        Key::Named(name) => (*name).to_owned(),
    });
    parts.join("+")
}

/// The keys the application handles itself, and what they do.
const APPLICATION_SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl + N", "new"),
    ("Ctrl + Shift + N", "new-window"),
    ("Ctrl + O", "open"),
    ("Ctrl + S", "save"),
    ("Ctrl + Shift + S", "save-as"),
    ("Ctrl + P", "print"),
    ("Ctrl + F", "find"),
    ("Ctrl + Shift + F", "find-in-project"),
    ("Ctrl + +", "zoom-in"),
    ("Ctrl + -", "zoom-out"),
    ("Ctrl + 0", "zoom-reset"),
];

/// The find bar's text field.
///
/// A stable id so opening the bar can put the caret in it. Without one the
/// bar appears and the next thing typed goes into the document, which is not
/// what pressing Find asked for.
pub(crate) fn find_input() -> cosmic::widget::Id {
    static ID: std::sync::LazyLock<cosmic::widget::Id> =
        std::sync::LazyLock::new(cosmic::widget::Id::unique);
    ID.clone()
}
