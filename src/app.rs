// SPDX-License-Identifier: GPL-3.0-only

//! The application.
//!
//! # What Pencil holds, and what it does not
//!
//! It holds a document, a path, and the settings. It does not hold a cursor, a
//! selection, an undo stack, a clipboard, or any idea of what a paragraph is —
//! all of that lives in the engine's [`EditorState`], and the widget hands
//! back a transaction for the application to apply. That division is the point
//! of the engine: an application built on it is mostly a file dialog and a
//! toolbar.

use std::path::PathBuf;

use cosmic::app::{Core, Task};
use cosmic::cosmic_config::CosmicConfigEntry as _;
use cosmic::iced::{Length, Subscription};
use cosmic::prelude::*;
use cosmic::widget::{self, nav_bar};

use nib::{Action, editor};
use nib_highlight::Highlighter;
use nib_model::decoration::DecorationSet;
use nib_model::input_rules::{self, InputRule};
use nib_model::keymap::Keymap;
use nib_model::schema::Schema;
use nib_model::search::{self, Heading, Matching, Query};
use nib_model::state::{EditorState, Selection};
use nib_model::{Transaction, basic, commands, history};

use crate::config::{CaretShape, Config};
use crate::document::{Converters, Format};
use crate::fl;

/// The application's unique identifier.
pub const APP_ID: &str = "com.magnetaros.Pencil";

#[derive(Clone, Debug)]
pub enum Message {
    /// Everything the editor widget does.
    Edit(Action),
    /// A named command, from a toolbar button or a menu.
    Command(&'static str),
    /// A heading level, or zero for a paragraph.
    Block(&'static str, i64),

    New,
    Open,
    Opened(PathBuf, Format, String),
    Save,
    SaveAs,
    SaveTo(PathBuf, Format),
    Saved(PathBuf),
    Failed(String),
    DismissError,

    ToggleFind,
    FindChanged(String),
    ReplaceChanged(String),
    FindNext,
    FindPrevious,
    ReplaceOne,
    ReplaceAll,
    CycleMatching,

    ToggleOutline,
    GoToHeading(usize),

    OpenContext(ContextPage),
    CloseContext,
    SetTextSize(u16),
    SetCaret(CaretShape),
    SetCaretBlinks(bool),
    SetCaretGlides(bool),
    SetMeasure(u16),

    ConfigChanged(Config),
}

/// The panes that open in the context drawer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextPage {
    Settings,
    Shortcuts,
    About,
}

impl ContextPage {
    fn title(self) -> String {
        match self {
            Self::Settings => fl!("settings"),
            Self::Shortcuts => fl!("shortcuts"),
            Self::About => fl!("about"),
        }
    }
}

/// The find bar's state.
#[derive(Debug, Default, Clone)]
pub(crate) struct Find {
    pub(crate) query: String,
    pub(crate) replacement: String,
    pub(crate) matching: Matching,
    pub(crate) hits: Vec<search::Hit>,
    pub(crate) current: Option<usize>,
    pub(crate) error: Option<String>,
}

pub struct App {
    core: Core,
    config: Config,
    config_handler: Option<cosmic::cosmic_config::Config>,

    schema: Schema,
    state: EditorState,
    keymap: Keymap,
    rules: Vec<InputRule>,
    converters: Converters,
    highlighter: Highlighter,

    /// Syntax colours and search hits, recomputed when the document changes.
    decorations: DecorationSet,
    /// The document as it stood when the highlighter last ran.
    highlighted: Option<nib_model::Node>,

    path: Option<PathBuf>,
    format: Format,
    dirty: bool,

    find: Option<Find>,
    outline: Vec<Heading>,
    nav: nav_bar::Model,
    error: Option<String>,
    context: Option<ContextPage>,
}

impl App {
    fn apply(&mut self, tr: Transaction) {
        if tr.doc_changed() {
            self.dirty = true;
        }
        self.state = self.state.applied(tr);
        self.refresh();
    }

    /// Recomputes everything derived from the document.
    fn refresh(&mut self) {
        let doc = self.state.doc().clone();
        if self.highlighted.as_ref() != Some(&doc) {
            self.highlighted = Some(doc.clone());
            self.outline = search::outline(&doc);
            self.rebuild_nav();
            if let Some(find) = &mut self.find
                && let Ok(query) = Query::new(&find.query, find.matching)
            {
                find.hits = search::find(&doc, &query);
                find.current = find
                    .current
                    .filter(|i| *i < find.hits.len())
                    .or_else(|| search::next_from(&find.hits, 0));
            }
        }
        self.decorations = self.build_decorations();
    }

    fn build_decorations(&self) -> DecorationSet {
        let mut all = self.highlighter.decorate(self.state.doc());
        if let Some(find) = &self.find
            && !find.hits.is_empty()
        {
            all = all.with(
                search::decorations(&find.hits, find.current)
                    .all()
                    .to_vec(),
            );
        }
        all
    }

    fn rebuild_nav(&mut self) {
        self.nav.clear();
        for (index, heading) in self.outline.iter().enumerate() {
            // Nesting is spelled with indentation rather than a tree: an
            // outline the reader has to expand is one they stop using.
            let indent = "    ".repeat(usize::try_from(heading.level.max(1) - 1).unwrap_or(0));
            let text = if heading.text.trim().is_empty() {
                fl!("untitled-heading")
            } else {
                heading.text.clone()
            };
            self.nav
                .insert()
                .text(format!("{indent}{text}"))
                .data::<usize>(index);
        }
    }

    fn load(&mut self, path: PathBuf, format: Format, source: &str) {
        let doc = self.converters.parse(format, source);
        self.state = fresh_state(&self.schema, doc);
        self.path = Some(path);
        self.format = format;
        self.dirty = false;
        self.error = None;
        self.highlighted = None;
        self.refresh();
    }

    fn save_to(&self, path: PathBuf, format: Format) -> Task<Message> {
        let contents = self.converters.write(format, self.state.doc());
        cosmic::task::future(async move {
            match crate::document::write(path, contents).await {
                Ok(path) => Message::Saved(path),
                Err(error) => Message::Failed(error.to_string()),
            }
        })
    }

    /// The style, resolved against the live theme and the settings.
    fn style(&self) -> nib::Style {
        self.config.style(&cosmic::theme::active())
    }

    fn run(&mut self, command: &commands::Command) {
        if let Some(tr) = command(&self.state) {
            self.apply(tr);
        }
    }
}

/// The schema's name for a toolbar command's mark.
fn mark_name(command: &str) -> &str {
    match command {
        "bold" => basic::marks::STRONG,
        "italic" => basic::marks::EM,
        "underline" => basic::marks::UNDERLINE,
        "strikethrough" => basic::marks::STRIKETHROUGH,
        other => other,
    }
}

/// A state over a document, with the history and input rules installed.
fn fresh_state(schema: &Schema, doc: nib_model::Node) -> EditorState {
    let selection = Selection::at_start(&doc);
    EditorState::with_selection(
        schema.clone(),
        doc,
        selection,
        vec![
            history::history(history::Options::default()),
            input_rules::input_rules_plugin(),
        ],
    )
}

impl cosmic::Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = Option<PathBuf>;
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Self::Flags) -> (Self, Task<Message>) {
        let schema = basic::schema();
        let handler = Config::handler().ok();
        let config = handler
            .as_ref()
            .map(|h| Config::get_entry(h).unwrap_or_else(|(_, config)| config))
            .unwrap_or_default();

        let mut app = Self {
            core,
            config,
            config_handler: handler,
            keymap: Keymap::base(&schema),
            rules: input_rules::base(&schema),
            converters: Converters::new(&schema),
            highlighter: Highlighter::new(),
            state: fresh_state(&schema, schema.empty_doc()),
            schema,
            decorations: DecorationSet::empty(),
            highlighted: None,
            path: None,
            format: Format::default(),
            dirty: false,
            find: None,
            outline: Vec::new(),
            nav: nav_bar::Model::default(),
            error: None,
            context: None,
        };
        app.refresh();

        let task = match flags {
            Some(path) => cosmic::task::future(async move {
                match crate::document::read(path).await {
                    Ok((path, format, source)) => Message::Opened(path, format, source),
                    Err(error) => Message::Failed(error.to_string()),
                }
            }),
            None => Task::none(),
        };
        (app, task)
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        // Buttons rather than a menu bar: COSMIC puts the common actions in
        // the header and everything else in a context drawer, and an editor's
        // common actions are these three.
        let button = |icon: &'static str, tooltip: String, message: Message| {
            widget::tooltip(
                widget::button::icon(widget::icon::from_name(icon).size(16)).on_press(message),
                widget::text::body(tooltip),
                widget::tooltip::Position::Bottom,
            )
            .into()
        };
        vec![
            button("document-new-symbolic", fl!("new"), Message::New),
            button("document-open-symbolic", fl!("open"), Message::Open),
            button("document-save-symbolic", fl!("save"), Message::Save),
            button(
                "document-save-as-symbolic",
                fl!("save-as"),
                Message::SaveAs,
            ),
        ]
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        let button = |icon: &'static str, tooltip: String, message: Message| {
            widget::tooltip(
                widget::button::icon(widget::icon::from_name(icon).size(16)).on_press(message),
                widget::text::body(tooltip),
                widget::tooltip::Position::Bottom,
            )
            .into()
        };
        vec![
            button("edit-find-symbolic", fl!("find"), Message::ToggleFind),
            button(
                "view-list-symbolic",
                fl!("outline"),
                Message::ToggleOutline,
            ),
            button(
                "preferences-system-symbolic",
                fl!("settings"),
                Message::OpenContext(ContextPage::Settings),
            ),
            button(
                "input-keyboard-symbolic",
                fl!("shortcuts"),
                Message::OpenContext(ContextPage::Shortcuts),
            ),
            button(
                "help-about-symbolic",
                fl!("about"),
                Message::OpenContext(ContextPage::About),
            ),
        ]
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        self.config.outline.then_some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        self.nav.activate(id);
        if let Some(index) = self.nav.data::<usize>(id).copied() {
            return self.update(Message::GoToHeading(index));
        }
        Task::none()
    }

    fn context_drawer(&self) -> Option<cosmic::app::context_drawer::ContextDrawer<'_, Message>> {
        let page = self.context?;
        Some(cosmic::app::context_drawer::context_drawer(
            match page {
                ContextPage::Settings => self.settings_page(),
                ContextPage::Shortcuts => self.shortcuts_page(),
                ContextPage::About => Self::about_page(),
            },
            Message::CloseContext,
        )
        .title(page.title()))
    }

    fn subscription(&self) -> Subscription<Message> {
        // The settings are shared with every other COSMIC application's
        // configuration store, so a change made elsewhere arrives here rather
        // than being noticed on the next launch.
        cosmic::cosmic_config::config_subscription::<_, Config>(
            std::any::TypeId::of::<Config>(),
            Self::APP_ID.into(),
            Config::VERSION,
        )
        .map(|update| Message::ConfigChanged(update.config))
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Edit(Action::Edit(tr)) => self.apply(*tr),
            Message::Edit(Action::Link(href)) => {
                // Opening a link is the desktop's business, not this
                // application's; `open` hands it to the portal.
                let _ = open::that_detached(&href);
            }
            Message::Edit(_) => {}

            Message::Command(name) => {
                if let Some(tr) = nib::toolbar_command(&self.state, name) {
                    self.apply(tr);
                }
            }
            Message::Block(name, level) => {
                let Some(typ) = self.schema.node_id(name) else {
                    return Task::none();
                };
                let attrs = (level > 0).then(|| nib_model::attrs! { "level" => level });
                self.run(&commands::set_block_type(typ, attrs));
            }

            Message::New => {
                self.state = fresh_state(&self.schema, self.schema.empty_doc());
                self.path = None;
                self.dirty = false;
                self.highlighted = None;
                self.refresh();
            }
            Message::Open => {
                return cosmic::task::future(async {
                    let dialog = cosmic::dialog::file_chooser::open::Dialog::new()
                        .title(fl!("open"));
                    match dialog.open_file().await {
                        Ok(response) => match response.url().to_file_path() {
                            Ok(path) => match crate::document::read(path).await {
                                Ok((path, format, source)) => {
                                    Message::Opened(path, format, source)
                                }
                                Err(error) => Message::Failed(error.to_string()),
                            },
                            Err(()) => Message::Failed(fl!("not-a-file")),
                        },
                        // A cancelled dialog is not an error; it is an answer.
                        Err(_) => Message::DismissError,
                    }
                });
            }
            Message::Opened(path, format, source) => self.load(path, format, &source),
            Message::Save => {
                let Some(path) = self.path.clone() else {
                    return self.update(Message::SaveAs);
                };
                return self.save_to(path, self.format);
            }
            Message::SaveAs => {
                let filters: Vec<String> = Format::all()
                    .iter()
                    .map(|f| format!("{f} (*.{})", f.extension()))
                    .collect();
                let suggested = format!(
                    "{}.{}",
                    self.path
                        .as_ref()
                        .and_then(|p| p.file_stem())
                        .and_then(|s| s.to_str())
                        .unwrap_or("document"),
                    self.format.extension()
                );
                return cosmic::task::future(async move {
                    let mut dialog = cosmic::dialog::file_chooser::save::Dialog::new()
                        .title(fl!("save-as"))
                        .file_name(suggested);
                    for (format, label) in Format::all().iter().zip(&filters) {
                        dialog = dialog.filter(
                            cosmic::dialog::file_chooser::FileFilter::new(label)
                                .glob(&format!("*.{}", format.extension())),
                        );
                    }
                    match dialog.save_file().await {
                        Ok(response) => match response.url().and_then(|u| u.to_file_path().ok()) {
                            Some(path) => {
                                let format = Format::of(&path);
                                Message::SaveTo(path, format)
                            }
                            None => Message::Failed(fl!("not-a-file")),
                        },
                        // A cancelled dialog is not an error; it is an answer.
                        Err(_) => Message::DismissError,
                    }
                });
            }
            Message::SaveTo(path, format) => {
                self.format = format;
                self.path = Some(path.clone());
                return self.save_to(path, format);
            }
            Message::Saved(path) => {
                self.path = Some(path);
                self.dirty = false;
                self.error = None;
            }
            Message::Failed(message) => self.error = Some(message),
            Message::DismissError => self.error = None,

            Message::ToggleFind => {
                self.find = match self.find {
                    Some(_) => None,
                    None => Some(Find::default()),
                };
                self.refresh();
            }
            Message::FindChanged(query) => {
                if let Some(find) = &mut self.find {
                    find.query = query;
                    match Query::new(&find.query, find.matching) {
                        Ok(compiled) => {
                            find.error = None;
                            find.hits = search::find(self.state.doc(), &compiled);
                            find.current = search::next_from(
                                &find.hits,
                                self.state.selection().from(),
                            );
                        }
                        Err(error) => {
                            find.error = Some(error.to_string());
                            find.hits.clear();
                            find.current = None;
                        }
                    }
                    self.decorations = self.build_decorations();
                }
            }
            Message::ReplaceChanged(text) => {
                if let Some(find) = &mut self.find {
                    find.replacement = text;
                }
            }
            Message::CycleMatching => {
                if let Some(find) = &mut self.find {
                    find.matching = match find.matching {
                        Matching::Loose => Matching::CaseSensitive,
                        Matching::CaseSensitive => Matching::WholeWord,
                        Matching::WholeWord => Matching::Regex,
                        Matching::Regex => Matching::Loose,
                    };
                    let query = find.query.clone();
                    return self.update(Message::FindChanged(query));
                }
            }
            Message::FindNext | Message::FindPrevious => {
                let forward = matches!(message, Message::FindNext);
                let Some(find) = &self.find else {
                    return Task::none();
                };
                let selection = self.state.selection();
                let index = if forward {
                    search::next_from(&find.hits, selection.to())
                } else {
                    search::previous_from(&find.hits, selection.from())
                };
                let Some(index) = index else {
                    return Task::none();
                };
                let hit = find.hits[index];
                if let Some(find) = &mut self.find {
                    find.current = Some(index);
                }
                let tr = search::select(&self.state, hit);
                self.apply(tr);
            }
            Message::ReplaceOne => {
                let Some(find) = self.find.clone() else {
                    return Task::none();
                };
                let Some(hit) = find.current.and_then(|i| find.hits.get(i).copied()) else {
                    return Task::none();
                };
                if let Some(tr) = search::replace(&self.state, hit, &find.replacement) {
                    self.apply(tr);
                    let query = find.query.clone();
                    return self.update(Message::FindChanged(query));
                }
            }
            Message::ReplaceAll => {
                let Some(find) = self.find.clone() else {
                    return Task::none();
                };
                let Ok(query) = Query::new(&find.query, find.matching) else {
                    return Task::none();
                };
                if let Some(tr) = search::replace_all(&self.state, &query, &find.replacement) {
                    self.apply(tr);
                    let text = find.query.clone();
                    return self.update(Message::FindChanged(text));
                }
            }

            Message::ToggleOutline => {
                self.config.outline = !self.config.outline;
                self.write_config();
            }
            Message::GoToHeading(index) => {
                let Some(heading) = self.outline.get(index) else {
                    return Task::none();
                };
                let mut tr = self.state.tr();
                tr.set_selection(Selection::cursor(heading.pos));
                self.apply(tr.clone().scroll_into_view());
            }

            Message::OpenContext(page) => {
                self.context = Some(page);
                self.core.window.show_context = true;
            }
            Message::CloseContext => {
                self.context = None;
                self.core.window.show_context = false;
            }
            Message::SetTextSize(size) => {
                self.config.text_size =
                    size.clamp(crate::config::MINIMUM_TEXT_SIZE, crate::config::MAXIMUM_TEXT_SIZE);
                self.write_config();
            }
            Message::SetCaret(shape) => {
                self.config.caret = shape.into();
                self.write_config();
            }
            Message::SetCaretBlinks(on) => {
                self.config.caret_blinks = on;
                self.write_config();
            }
            Message::SetCaretGlides(on) => {
                self.config.caret_glides = on;
                self.write_config();
            }
            Message::SetMeasure(measure) => {
                self.config.measure = measure;
                self.write_config();
            }
            Message::ConfigChanged(config) => self.config = config,
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        let document = editor(&self.state)
            .decorations(&self.decorations)
            .keymap(&self.keymap)
            .input_rules(&self.rules)
            .style(self.style())
            .autofocus()
            .on_action(Message::Edit);

        // The measure: a line of prose past about eighty characters is one the
        // eye loses its place returning from, and a maximised window is
        // exactly how that happens.
        let column = match self.config.column_width() {
            Some(width) => widget::container(document)
                .max_width(width)
                .width(Length::Fill)
                .into(),
            None => Element::from(document),
        };

        let page = widget::container(
            widget::scrollable(widget::container(column).center_x(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill),
        );

        let mut screen = widget::column::with_capacity(5).push(self.toolbar());
        if let Some(find) = &self.find {
            screen = screen.push(Self::find_bar(find));
        }
        if let Some(error) = &self.error {
            screen = screen.push(Self::error_bar(error));
        }
        screen
            .push(page.height(Length::Fill))
            .push(self.status_bar())
            .spacing(spacing.space_none)
            .into()
    }
}

impl App {
    // -- what the chrome needs to see -------------------------------------

    pub(crate) fn state(&self) -> &EditorState {
        &self.state
    }

    pub(crate) fn schema(&self) -> &Schema {
        &self.schema
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    pub(crate) fn format(&self) -> Format {
        self.format
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether a mark is on where the caret is, so its button can be shown
    /// pressed.
    ///
    /// The pending marks first: press Ctrl+B on a caret and the button lights
    /// before a single character has been typed, which is what tells you the
    /// press landed.
    pub(crate) fn mark_is_on(&self, name: &str) -> bool {
        let Some(id) = self.schema.mark_id(mark_name(name)) else {
            return false;
        };
        let selection = self.state.selection();
        if selection.is_empty() {
            return self.state.marks().iter().any(|m| m.typ().id() == id);
        }
        self.state
            .doc()
            .range_has_mark(selection.from(), selection.to(), id)
    }

    fn write_config(&mut self) {
        if let Some(handler) = &self.config_handler
            && let Err(errors) = self.config.write_entry(handler)
        {
            // A setting that will not persist is worth saying once, in the
            // place errors go, rather than swallowing.
            self.error = Some(format!("{errors:?}"));
        }
    }
}
