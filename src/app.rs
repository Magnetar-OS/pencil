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
use cosmic::Application as _;
use cosmic::prelude::*;
use cosmic::widget::{self, nav_bar, segmented_button, tab_bar};

use nib::{Action, editor};
use nib_highlight::Highlighter;
use nib_spell::Speller;
use nib_model::decoration::DecorationSet;
use nib_model::input_rules::{self, InputRule};
use nib_model::keymap::Keymap;
use nib_model::schema::Schema;
use nib_model::search::{self, Heading, Matching, Query};
use nib_model::state::{EditorState, Selection};
use nib_model::{Transaction, basic, commands};

use crate::config::{Appearance, CaretShape, Config};
use crate::document::{Converters, Document, Format};
use crate::project::Project;
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

    /// The user asked for something that would discard unsaved work.
    Confirm(Pending),
    /// Save first, then do it.
    ConfirmSave,
    /// Do it anyway.
    ConfirmDiscard,
    /// Do nothing after all.
    ConfirmCancel,

    ZoomIn,
    ZoomOut,
    ZoomReset,
    SetTheme(Appearance),
    OpenRecent(PathBuf),
    ClearRecent,

    TabActivate(segmented_button::Entity),
    TabClose(segmented_button::Entity),

    OpenFolder,
    FolderOpened(PathBuf),
    CloseFolder,
    ShowSidebar(Sidebar),

    SetLineNumbers(bool),
    SetWrapCode(bool),
    SetSpellCheck(bool),
    SetSpellLanguage(String),
    /// Replace the misspelling under the caret with a suggestion.
    Correct(usize, usize, String),
    /// Teach the dictionary the word under the caret.
    Learn(String),

    /// A clipboard action from the menu. The widget handles the shortcuts.
    Clipboard(&'static str),
    /// What the system clipboard held, on the way to being pasted.
    Pasted(Option<String>),
}

/// What is waiting on the unsaved-changes question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    New,
    Open,
    OpenPath(PathBuf),
    CloseTab(segmented_button::Entity),
}

/// The panes that open in the context drawer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextPage {
    Settings,
    Shortcuts,
    Statistics,
    Recent,
    About,
}

impl ContextPage {
    fn title(self) -> String {
        match self {
            Self::Settings => fl!("settings"),
            Self::Shortcuts => fl!("shortcuts"),
            Self::Statistics => fl!("statistics"),
            Self::Recent => fl!("recent"),
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

/// What the sidebar is showing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Sidebar {
    /// The active document's headings.
    #[default]
    Outline,
    /// The open folder.
    Project,
}

/// What a sidebar row points at.
#[derive(Clone, Copy, Debug)]
enum NavTarget {
    Heading(usize),
    Entry(usize),
}

pub struct App {
    core: Core,
    config: Config,
    config_handler: Option<cosmic::cosmic_config::Config>,

    schema: Schema,
    keymap: Keymap,
    rules: Vec<InputRule>,
    converters: Converters,
    highlighter: Highlighter,
    /// The spell checker, when a dictionary for the chosen language is
    /// installed. `None` is the normal off state, not a fault.
    speller: Option<Speller>,

    /// The open documents. Each tab holds one as its data.
    tabs: segmented_button::SingleSelectModel,

    project: Option<Project>,
    sidebar: Sidebar,

    find: Option<Find>,
    nav: nav_bar::Model,
    error: Option<String>,
    context: Option<ContextPage>,
    /// What the unsaved-changes dialog is asking about.
    pending: Option<Pending>,
    /// What to carry on with once a save the user asked for completes.
    after_save: Option<Pending>,
}

impl App {
    /// The active document.
    ///
    /// # Panics
    ///
    /// Never: a tab is opened at startup and the last one cannot be closed,
    /// so there is always exactly one active.
    pub(crate) fn doc(&self) -> &Document {
        self.tabs
            .active_data::<Document>()
            .expect("there is always an active document")
    }

    fn doc_mut(&mut self) -> &mut Document {
        self.tabs
            .active_data_mut::<Document>()
            .expect("there is always an active document")
    }

    /// Opens a document in a new tab and activates it.
    fn add_tab(&mut self, document: Document) {
        let title = document.title();
        let id = self
            .tabs
            .insert()
            .text(title)
            .closable()
            .data(document)
            .id();
        self.tabs.activate(id);
        self.refresh();
    }

    /// Keeps the tab's label in step with its document.
    fn retitle(&mut self) {
        let Some(id) = self.tabs.active_data::<Document>().map(|_| self.tabs.active())
        else {
            return;
        };
        let title = self.doc().title();
        let modified = if self.doc().dirty {
            format!("• {title}")
        } else {
            title
        };
        self.tabs.text_set(id, modified);
    }

    /// Puts a path at the head of the recent list.
    fn remember(&mut self, path: &std::path::Path) {
        self.config.remember(path);
        self.write_config();
    }

    /// Carries on with what the unsaved-changes question was blocking.
    fn resume(&mut self, pending: Pending) -> Task<Message> {
        match pending {
            Pending::New => self.update(Message::New),
            Pending::Open => self.update(Message::Open),
            Pending::OpenPath(path) => open_path(path),
            Pending::CloseTab(id) => {
                self.close_tab(id);
                Task::none()
            }
        }
    }

    /// Removes a tab and activates a neighbour.
    fn close_tab(&mut self, id: segmented_button::Entity) {
        let position = self.tabs.position(id);
        self.tabs.remove(id);
        // The tab to the left, or the first one — whichever still exists.
        let next = position
            .and_then(|p| self.tabs.entity_at(p.saturating_sub(1)))
            .or_else(|| self.tabs.iter().next());
        if let Some(next) = next {
            self.tabs.activate(next);
        }
        self.find = None;
        self.refresh();
        self.rebuild_nav();
    }

    fn apply(&mut self, tr: Transaction) {
        if tr.doc_changed() {
            self.doc_mut().dirty = true;
        }
        let next = self.doc().state.applied(tr);
        self.doc_mut().state = next;
        self.refresh();
        self.retitle();
    }

    /// Recomputes everything derived from the document.
    fn refresh(&mut self) {
        let doc = self.doc().state.doc().clone();
        if self.doc().derived_from.as_ref() != Some(&doc) {
            self.doc_mut().derived_from = Some(doc.clone());
            let outline = search::outline(&doc);
            self.doc_mut().outline = outline;
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
        let decorations = self.build_decorations();
        self.doc_mut().decorations = decorations;
    }

    /// Loads the dictionary the settings ask for, and teaches it what the
    /// user has taught it before.
    fn reload_speller(&mut self) {
        if !self.config.spell_check {
            self.speller = None;
            return;
        }
        let language = self.config.dictionary();
        self.speller = match Speller::system(&language) {
            Ok(mut speller) => {
                for word in &self.config.learnt_words {
                    speller.learn(word);
                }
                Some(speller)
            }
            // No dictionary installed is the normal off state: no squiggles,
            // and a settings pane that says which package would turn it on.
            Err(_) => None,
        };
    }

    fn build_decorations(&self) -> DecorationSet {
        let mut all = self.highlighter.decorate(self.doc().state.doc());
        if let Some(speller) = &self.speller {
            all = all.with(speller.decorate(self.doc().state.doc()).all().to_vec());
        }
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
        let rows: Vec<(usize, String, i64)> = match self.sidebar {
            Sidebar::Outline => self
                .doc()
                .outline
                .iter()
                .enumerate()
                .map(|(i, h)| (i, h.text.clone(), h.level))
                .collect(),
            Sidebar::Project => Vec::new(),
        };
        if self.sidebar == Sidebar::Project {
            self.rebuild_project_nav();
            return;
        }
        for (index, text, level) in rows {
            let heading = Heading {
                pos: 0,
                level,
                text,
            };
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
                .data(NavTarget::Heading(index));
        }
    }

    /// The sidebar, showing the open folder.
    fn rebuild_project_nav(&mut self) {
        let Some(project) = &self.project else {
            return;
        };
        let rows: Vec<(usize, String, usize, bool, bool, Option<crate::project::Status>)> =
            project
                .entries()
                .iter()
                .enumerate()
                .map(|(i, e)| (i, e.name(), e.depth, e.is_dir, e.expanded, e.status))
                .collect();
        for (index, name, depth, is_dir, expanded, status) in rows {
            let indent = "  ".repeat(depth);
            let arrow = if is_dir {
                if expanded { "▾ " } else { "▸ " }
            } else {
                ""
            };
            let marker = status.map_or("", crate::project::Status::marker);
            let label = if marker.is_empty() {
                format!("{indent}{arrow}{name}")
            } else {
                format!("{indent}{arrow}{name}  {marker}")
            };
            self.nav.insert().text(label).data(NavTarget::Entry(index));
        }
    }

    /// Opens a file, reusing the tab when the one showing is empty and
    /// untouched — a new window should not leave a blank tab behind.
    fn load(&mut self, path: PathBuf, format: Format, source: &str) {
        let node = self.converters.parse(format, source);
        let document = Document::over(&self.schema, node, Some(path), format);
        self.error = None;

        let replaceable = self.doc().path.is_none()
            && !self.doc().dirty
            && self.doc().state.doc().text_content().trim().is_empty();
        if replaceable {
            *self.doc_mut() = document;
            self.refresh();
            self.retitle();
        } else {
            self.add_tab(document);
            self.retitle();
        }
    }

    fn save_to(&self, path: PathBuf, format: Format) -> Task<Message> {
        let contents = self.converters.write(format, self.doc().state.doc());
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
        if let Some(tr) = command(&self.doc().state) {
            self.apply(tr);
        }
    }
}

/// Reads a file and turns the result into a message.
fn open_path(path: PathBuf) -> Task<Message> {
    cosmic::task::future(async move {
        match crate::document::read(path).await {
            Ok((path, format, source)) => Message::Opened(path, format, source),
            Err(error) => Message::Failed(error.to_string()),
        }
    })
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

        let mut tabs = segmented_button::SingleSelectModel::default();
        let first = tabs
            .insert()
            .text(fl!("untitled"))
            .closable()
            .data(Document::empty(&schema))
            .id();
        tabs.activate(first);

        let mut app = Self {
            core,
            config,
            config_handler: handler,
            keymap: Keymap::base(&schema),
            rules: input_rules::base(&schema),
            converters: Converters::new(&schema),
            highlighter: Highlighter::new(),
            speller: None,
            tabs,
            schema,
            project: None,
            sidebar: Sidebar::default(),
            find: None,
            nav: nav_bar::Model::default(),
            error: None,
            context: None,
            pending: None,
            after_save: None,
        };
        app.reload_speller();
        app.refresh();

        let theme = cosmic::command::set_theme(app.config.appearance().theme());
        let task = match flags {
            Some(path) => cosmic::task::future(async move {
                match crate::document::read(path).await {
                    Ok((path, format, source)) => Message::Opened(path, format, source),
                    Err(error) => Message::Failed(error.to_string()),
                }
            }),
            None => Task::none(),
        };
        (app, Task::batch([theme, task]))
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
            button(
                "folder-open-symbolic",
                fl!("open-folder"),
                Message::OpenFolder,
            ),
            button(
                if self.sidebar == Sidebar::Project {
                    "view-list-symbolic"
                } else {
                    "folder-symbolic"
                },
                if self.sidebar == Sidebar::Project {
                    fl!("outline")
                } else {
                    fl!("project")
                },
                Message::ShowSidebar(if self.sidebar == Sidebar::Project {
                    Sidebar::Outline
                } else {
                    Sidebar::Project
                }),
            ),
            button(
                "document-open-recent-symbolic",
                fl!("recent"),
                Message::OpenContext(ContextPage::Recent),
            ),
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
                "accessories-calculator-symbolic",
                fl!("statistics"),
                Message::OpenContext(ContextPage::Statistics),
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
        match self.nav.data::<NavTarget>(id).copied() {
            Some(NavTarget::Heading(index)) => self.update(Message::GoToHeading(index)),
            Some(NavTarget::Entry(index)) => {
                let Some(project) = &mut self.project else {
                    return Task::none();
                };
                let Some(entry) = project.entries().get(index) else {
                    return Task::none();
                };
                if entry.is_dir {
                    project.toggle(index);
                    self.rebuild_nav();
                    return Task::none();
                }
                let path = entry.path.clone();
                // A file already open is switched to rather than opened twice.
                let existing = self.tabs.iter().find(|id| {
                    self.tabs
                        .data::<Document>(*id)
                        .and_then(|d| d.path.as_deref())
                        == Some(path.as_path())
                });
                match existing {
                    Some(id) => self.update(Message::TabActivate(id)),
                    None => open_path(path),
                }
            }
            None => Task::none(),
        }
    }

    fn context_drawer(&self) -> Option<cosmic::app::context_drawer::ContextDrawer<'_, Message>> {
        let page = self.context?;
        Some(cosmic::app::context_drawer::context_drawer(
            match page {
                ContextPage::Settings => self.settings_page(),
                ContextPage::Shortcuts => self.shortcuts_page(),
                ContextPage::Statistics => self.statistics_page(),
                ContextPage::Recent => self.recent_page(),
                ContextPage::About => Self::about_page(),
            },
            Message::CloseContext,
        )
        .title(page.title()))
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let pending = self.pending.as_ref()?;
        let _ = pending;
        Some(
            widget::dialog()
                .title(fl!("unsaved-title", name = self.doc().title()))
                .body(fl!("unsaved-body"))
                .primary_action(
                    widget::button::suggested(fl!("save-changes"))
                        .on_press(Message::ConfirmSave),
                )
                .secondary_action(
                    widget::button::destructive(fl!("discard-changes"))
                        .on_press(Message::ConfirmDiscard),
                )
                .tertiary_action(
                    widget::button::text(fl!("cancel")).on_press(Message::ConfirmCancel),
                )
                .into(),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        // The settings are shared with every other COSMIC application's
        // configuration store, so a change made elsewhere arrives here rather
        // than being noticed on the next launch.
        Subscription::batch([
            cosmic::cosmic_config::config_subscription::<_, Config>(
                std::any::TypeId::of::<Config>(),
                Self::APP_ID.into(),
                Config::VERSION,
            )
            .map(|update| Message::ConfigChanged(update.config)),
            // Zoom and the file shortcuts are the application's, not the
            // document's: the editor's keymap produces transactions, and none
            // of these is one.
            cosmic::iced::event::listen_with(|event, _status, _window| {
                let cosmic::iced::Event::Keyboard(
                    cosmic::iced::keyboard::Event::KeyPressed {
                        key, modifiers, ..
                    },
                ) = event
                else {
                    return None;
                };
                if !modifiers.command() {
                    return None;
                }
                let cosmic::iced::keyboard::Key::Character(c) = key else {
                    return None;
                };
                match c.as_str() {
                    "=" | "+" => Some(Message::ZoomIn),
                    "-" => Some(Message::ZoomOut),
                    "0" => Some(Message::ZoomReset),
                    "s" if modifiers.shift() => Some(Message::SaveAs),
                    "s" => Some(Message::Save),
                    "o" => Some(Message::Open),
                    "n" => Some(Message::New),
                    "f" => Some(Message::ToggleFind),
                    _ => None,
                }
            }),
        ])
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Edit(Action::Edit(tr)) => self.apply(*tr),
            Message::Clipboard(what) => {
                let state = self.doc().state.clone();
                let selection = state.selection();
                match what {
                    "copy" | "cut" if !selection.is_empty() => {
                        // The same text the widget's own Ctrl+C writes: the
                        // engine's function, not a second reading of what a
                        // selection looks like.
                        let slice = selection.content(state.doc());
                        let text = nib::plain_text(&state, &slice);
                        if what == "cut" {
                            let mut tr = state.tr().now();
                            if tr.delete_selection().is_ok() {
                                self.apply(tr.clone());
                            }
                        }
                        return cosmic::iced::clipboard::write(text)
                            .map(cosmic::Action::App);
                    }
                    "paste" => {
                        return cosmic::iced::clipboard::read()
                            .map(|text| cosmic::Action::App(Message::Pasted(text)));
                    }
                    _ => {}
                }
            }
            Message::Pasted(text) => {
                let Some(text) = text.filter(|t| !t.is_empty()) else {
                    return Task::none();
                };
                let state = self.doc().state.clone();
                let slice = nib::parse_plain(&state, &text);
                let mut tr = state.tr().now();
                if tr.replace_selection(slice).is_ok() {
                    self.apply(tr.clone());
                }
            }
            Message::Edit(Action::Link(href)) => {
                // Opening a link is the desktop's business, not this
                // application's; `open` hands it to the portal.
                let _ = open::that_detached(&href);
            }
            // Everything else the widget reports needs nothing from here. A
            // right-click is the notable one: the widget has already moved the
            // caret, and the menu opens itself on the button's release.
            Message::Edit(_) => {}

            Message::Command(name) => {
                if let Some(tr) = nib::toolbar_command(&self.doc().state, name) {
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
                // Nothing that would lose work happens without asking. The
                // one thing a text editor must never do is throw away
                // something the user typed because they clicked the wrong
                // button.
                // A new document is a new tab, so nothing is discarded and
                // the guard has nothing to ask about.
                let document = Document::empty(&self.schema);
                self.add_tab(document);
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
            Message::Opened(path, format, source) => {
                self.remember(&path);
                self.load(path, format, &source);
            }

            Message::Confirm(pending) => self.pending = Some(pending),
            Message::ConfirmCancel => self.pending = None,
            Message::ConfirmSave => {
                // Save, and let the save's own completion carry on with what
                // was waiting.
                let pending = self.pending.take();
                self.after_save = pending;
                return self.update(Message::Save);
            }
            Message::ConfirmDiscard => {
                let Some(pending) = self.pending.take() else {
                    return Task::none();
                };
                self.doc_mut().dirty = false;
                return self.resume(pending);
            }

            Message::ZoomIn => {
                let size = self.config.text_size.saturating_add(1);
                return self.update(Message::SetTextSize(size));
            }
            Message::ZoomOut => {
                let size = self.config.text_size.saturating_sub(1);
                return self.update(Message::SetTextSize(size));
            }
            Message::ZoomReset => {
                return self.update(Message::SetTextSize(Config::default().text_size));
            }
            Message::SetTheme(appearance) => {
                self.config.appearance = appearance.into();
                self.write_config();
                return cosmic::command::set_theme(appearance.theme());
            }
            Message::OpenRecent(path) => {
                return open_path(path);
            }
            Message::ClearRecent => {
                self.config.recent.clear();
                self.write_config();
            }
            Message::Save => {
                let Some(path) = self.doc().path.clone() else {
                    return self.update(Message::SaveAs);
                };
                let format = self.doc().format;
                return self.save_to(path, format);
            }
            Message::SaveAs => {
                let filters: Vec<String> = Format::all()
                    .iter()
                    .map(|f| format!("{f} (*.{})", f.extension()))
                    .collect();
                let suggested = format!(
                    "{}.{}",
                    self.doc()
                        .path
                        .as_ref()
                        .and_then(|p| p.file_stem())
                        .and_then(|s| s.to_str())
                        .unwrap_or("document"),
                    self.doc().format.extension()
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
                self.doc_mut().format = format;
                self.doc_mut().path = Some(path.clone());
                return self.save_to(path, format);
            }
            Message::Saved(path) => {
                self.remember(&path);
                self.doc_mut().path = Some(path);
                self.doc_mut().dirty = false;
                self.error = None;
                self.retitle();
                if let Some(project) = &mut self.project {
                    project.refresh_status();
                }
                self.rebuild_nav();
                // A save that something was waiting on carries on with it.
                if let Some(pending) = self.after_save.take() {
                    return self.resume(pending);
                }
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
                let doc = self
                    .tabs
                    .active_data::<Document>()
                    .expect("there is always an active document");
                if let Some(find) = &mut self.find {
                    find.query = query;
                    match Query::new(&find.query, find.matching) {
                        Ok(compiled) => {
                            find.error = None;
                            find.hits = search::find(doc.state.doc(), &compiled);
                            find.current = search::next_from(
                                &find.hits,
                                doc.state.selection().from(),
                            );
                        }
                        Err(error) => {
                            find.error = Some(error.to_string());
                            find.hits.clear();
                            find.current = None;
                        }
                    }
                }
                let decorations = self.build_decorations();
                self.doc_mut().decorations = decorations;
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
                let selection = self.doc().state.selection();
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
                let tr = search::select(&self.doc().state, hit);
                self.apply(tr);
            }
            Message::ReplaceOne => {
                let Some(find) = self.find.clone() else {
                    return Task::none();
                };
                let Some(hit) = find.current.and_then(|i| find.hits.get(i).copied()) else {
                    return Task::none();
                };
                if let Some(tr) = search::replace(&self.doc().state, hit, &find.replacement) {
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
                if let Some(tr) =
                    search::replace_all(&self.doc().state, &query, &find.replacement)
                {
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
                let Some(heading) = self.doc().outline.get(index).cloned() else {
                    return Task::none();
                };
                let mut tr = self.doc().state.tr();
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

            Message::TabActivate(id) => {
                self.tabs.activate(id);
                self.find = None;
                self.refresh();
                self.rebuild_nav();
            }
            Message::TabClose(id) => {
                // The last tab is emptied rather than removed: a window with
                // no document in it is a window with nothing to do.
                if self.tabs.iter().count() <= 1 {
                    *self.doc_mut() = Document::empty(&self.schema);
                    self.retitle();
                    self.refresh();
                    return Task::none();
                }
                if self
                    .tabs
                    .data::<Document>(id)
                    .is_some_and(|document| document.dirty)
                {
                    self.tabs.activate(id);
                    self.pending = Some(Pending::CloseTab(id));
                    return Task::none();
                }
                self.close_tab(id);
            }

            Message::OpenFolder => {
                return cosmic::task::future(async {
                    let dialog = cosmic::dialog::file_chooser::open::Dialog::new()
                        .title(fl!("open-folder"));
                    match dialog.open_folder().await {
                        Ok(response) => match response.url().to_file_path() {
                            Ok(path) => Message::FolderOpened(path),
                            Err(()) => Message::Failed(fl!("not-a-file")),
                        },
                        Err(_) => Message::DismissError,
                    }
                });
            }
            Message::FolderOpened(path) => {
                self.project = Some(Project::open(path));
                self.sidebar = Sidebar::Project;
                self.config.outline = true;
                self.write_config();
                self.rebuild_nav();
            }
            Message::CloseFolder => {
                self.project = None;
                self.sidebar = Sidebar::Outline;
                self.rebuild_nav();
            }
            Message::ShowSidebar(which) => {
                self.sidebar = which;
                self.rebuild_nav();
            }

            Message::SetLineNumbers(on) => {
                self.config.line_numbers = on;
                self.write_config();
            }
            Message::SetWrapCode(on) => {
                self.config.wrap_code = on;
                self.write_config();
            }
            Message::SetSpellCheck(on) => {
                self.config.spell_check = on;
                self.write_config();
                self.reload_speller();
                self.refresh();
            }
            Message::SetSpellLanguage(language) => {
                self.config.spell_language = language;
                self.write_config();
                self.reload_speller();
                self.refresh();
            }
            Message::Correct(from, to, replacement) => {
                let state = self.doc().state.clone();
                let marks = state.doc().resolve(from).marks();
                let node = state.schema().text(replacement.as_str(), marks);
                let mut tr = state.tr().now();
                if tr
                    .replace(
                        from,
                        to,
                        nib_model::slice::Slice::new(
                            nib_model::fragment::Fragment::from(node),
                            0,
                            0,
                        ),
                    )
                    .is_ok()
                {
                    self.apply(tr.clone());
                }
            }
            Message::Learn(word) => {
                if let Some(speller) = &mut self.speller {
                    speller.learn(&word);
                }
                if !self.config.learnt_words.contains(&word) {
                    self.config.learnt_words.push(word);
                    self.write_config();
                }
                self.refresh();
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        let document = editor(&self.doc().state)
            .decorations(&self.doc().decorations)
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
        )
        .height(Length::Fill);
        // Always present: the widget opens it on the right button's *release*,
        // and the editor moves the caret on the press — so by the time the
        // menu appears it is about what was pointed at.
        let page = widget::context_menu(page, Some(self.context_menu()));

        let mut screen = widget::column::with_capacity(6);
        if self.tabs.iter().count() > 1 {
            screen = screen.push(
                tab_bar::horizontal(&self.tabs)
                    .on_activate(Message::TabActivate)
                    .on_close(Message::TabClose),
            );
        }
        screen = screen.push(self.toolbar());
        if let Some(find) = &self.find {
            screen = screen.push(Self::find_bar(find));
        }
        if let Some(error) = &self.error {
            screen = screen.push(Self::error_bar(error));
        }
        screen
            .push(page)
            .push(self.status_bar())
            .spacing(spacing.space_none)
            .into()
    }
}

impl App {
    // -- what the chrome needs to see -------------------------------------

    pub(crate) fn state(&self) -> &EditorState {
        &self.doc().state
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
        self.doc().path.as_deref()
    }

    pub(crate) fn format(&self) -> Format {
        self.doc().format
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.doc().dirty
    }

    pub(crate) fn speller(&self) -> Option<&Speller> {
        self.speller.as_ref()
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
        let selection = self.doc().state.selection();
        if selection.is_empty() {
            return self.doc().state.marks().iter().any(|m| m.typ().id() == id);
        }
        self.doc()
            .state
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
