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

use cosmic::Application as _;
use cosmic::app::{Core, Task};
use cosmic::cosmic_config::CosmicConfigEntry as _;
use cosmic::iced::{Length, Subscription};
use cosmic::prelude::*;
use cosmic::widget::{self, nav_bar, segmented_button, tab_bar};

use nib::{Action, editor};
use nib_highlight::Highlighter;
use nib_model::decoration::DecorationSet;
use nib_model::input_rules::{self, InputRule};
use nib_model::keymap::Keymap;
use nib_model::node::Node;
use nib_model::schema::Schema;
use nib_model::search::{self, Heading, Matching, Query};
use nib_model::state::{EditorState, Selection};
use nib_model::{Transaction, basic, commands};
use nib_spell::Speller;

use crate::config::{Appearance, CaretShape, Config};
use crate::document::{Converters, Document, Format};
use crate::fl;
use crate::launch::{self, Flags};
use crate::project::{self, Project};

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
    /// A second window, which is a second process. See the handler.
    NewWindow,
    Open,
    Opened(PathBuf, Format, String),
    Save,
    SaveAs,
    /// Hand the document to the desktop's print dialog.
    Print,
    SaveTo(PathBuf, Format),
    /// A write finished: which tab it was, where and how it was written, and
    /// the document exactly as it went to disk.
    Saved(segmented_button::Entity, PathBuf, Format, Node),
    /// The Save As dialog was dismissed.
    SaveCancelled,
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

    /// Search the open folder for what the find bar holds.
    FindInProject,
    /// What that search found.
    ProjectSearched(Vec<project::Hit>),
    /// Open the file a result points at, at the match.
    GoToResult(usize),

    OpenContext(ContextPage),
    CloseContext,
    SetTextSize(u16),
    SetCaret(CaretShape),
    SetCaretBlinks(bool),
    SetCaretGlides(bool),
    SetMeasure(u16),

    ConfigChanged(Config),

    /// Save first, then do it.
    ConfirmSave,
    /// Do it anyway.
    ConfirmDiscard,
    /// Do nothing after all.
    ConfirmCancel,
    /// The window was asked to close, by its own close button or by the
    /// desktop. Unsaved tabs are asked about first.
    CloseRequested,

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
    SetVim(bool),
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

    /// Nothing to do. What a task returns when its work was the point.
    Ignore,
}

/// What is waiting on the unsaved-changes question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    CloseTab(segmented_button::Entity),
    /// Closing the window. The tabs the user has already chosen to discard
    /// are listed, so a later Cancel leaves them unsaved rather than marked
    /// clean.
    Quit(Vec<segmented_button::Entity>),
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
    /// What the last folder-wide search found.
    Results,
}

/// What a folder-wide search found, and what it was looking for.
///
/// The query is kept because opening a result searches the parsed document
/// again: the file said which line, and the document says which position.
pub(crate) struct Results {
    pub(crate) query: String,
    pub(crate) matching: Matching,
    pub(crate) hits: Vec<project::Hit>,
    /// Whether the search stopped at [`project::SEARCH_LIMIT`].
    pub(crate) truncated: bool,
    /// Set while the search is running, so the sidebar can say so.
    pub(crate) running: bool,
}

/// What a sidebar row points at.
#[derive(Clone, Copy, Debug)]
enum NavTarget {
    Heading(usize),
    Entry(usize),
    Result(usize),
}

pub struct App {
    core: Core,
    config: Config,
    /// The settings as last read from or written to disk, so a write puts
    /// down only what this window changed.
    persisted: Config,
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
    /// The last folder-wide search.
    results: Option<Results>,
    /// A result waiting for its file to finish opening: the path, and which
    /// match in it to land on.
    jump: Option<(PathBuf, usize)>,

    find: Option<Find>,
    nav: nav_bar::Model,
    error: Option<String>,
    context: Option<ContextPage>,
    /// What the unsaved-changes dialog is asking about.
    pending: Option<Pending>,
    /// What to carry on with once a save the user asked for completes, and
    /// the tab whose save that is.
    after_save: Option<(segmented_button::Entity, Pending)>,
    /// The Vim mode, for the status bar. The widget owns the state machine;
    /// this is the last thing it said.
    mode: nib_model::vim::Mode,
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

    /// Keeps the active tab's label in step with its document.
    fn retitle(&mut self) {
        self.retitle_tab(self.tabs.active());
    }

    /// Keeps one tab's label in step with its document.
    fn retitle_tab(&mut self, id: segmented_button::Entity) {
        let Some(document) = self.tabs.data::<Document>(id) else {
            return;
        };
        let title = document.title();
        let modified = if document.dirty {
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
            Pending::CloseTab(id) => {
                self.close_tab(id);
                Task::none()
            }
            Pending::Quit(discarded) => self.quit(discarded),
        }
    }

    /// The next unsaved tab the user has not already chosen to discard.
    fn next_unsaved(
        &self,
        discarded: &[segmented_button::Entity],
    ) -> Option<segmented_button::Entity> {
        self.tabs.iter().find(|id| {
            !discarded.contains(id)
                && self
                    .tabs
                    .data::<Document>(*id)
                    .is_some_and(|document| document.dirty)
        })
    }

    /// Closes the window, asking about each unsaved tab first.
    ///
    /// One tab at a time, with the same Save / Discard / Cancel as closing a
    /// tab: an untitled document needs its own Save As, and a list of names
    /// with one "Save all" button cannot give it one. Cancel at any point
    /// keeps the window and every tab as it was.
    fn quit(&mut self, discarded: Vec<segmented_button::Entity>) -> Task<Message> {
        let Some(id) = self.next_unsaved(&discarded) else {
            return cosmic::iced::exit();
        };
        let task = self.update(Message::TabActivate(id));
        self.pending = Some(Pending::Quit(discarded));
        task
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
            all = all.with(search::decorations(&find.hits, find.current).all().to_vec());
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
            Sidebar::Project | Sidebar::Results => Vec::new(),
        };
        match self.sidebar {
            Sidebar::Project => {
                self.rebuild_project_nav();
                return;
            }
            Sidebar::Results => {
                self.rebuild_results_nav();
                return;
            }
            Sidebar::Outline => {}
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
        let rows: Vec<(
            usize,
            String,
            usize,
            bool,
            bool,
            Option<crate::project::Status>,
        )> = project
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

    /// The sidebar, showing what the last folder-wide search found.
    ///
    /// A file's name once, then its matching lines under it: a list that
    /// repeats the path on every row is a list whose rows are all path.
    fn rebuild_results_nav(&mut self) {
        let Some(results) = &self.results else {
            return;
        };
        if results.running {
            self.nav.insert().text(fl!("searching"));
            return;
        }
        let root = self.project.as_ref().map(|p| p.root().to_path_buf());
        let mut rows: Vec<(Option<String>, String, usize)> = Vec::new();
        let mut last: Option<PathBuf> = None;
        for (index, hit) in results.hits.iter().enumerate() {
            let heading = (last.as_deref() != Some(hit.path.as_path())).then(|| {
                let shown = root
                    .as_deref()
                    .and_then(|root| hit.path.strip_prefix(root).ok())
                    .unwrap_or(hit.path.as_path());
                shown.display().to_string()
            });
            last = Some(hit.path.clone());
            let text = if hit.text.len() > 80 {
                let cut = hit
                    .text
                    .char_indices()
                    .nth(80)
                    .map_or(hit.text.len(), |(i, _)| i);
                format!("{}\u{2026}", &hit.text[..cut])
            } else {
                hit.text.clone()
            };
            rows.push((heading, format!("  {}: {text}", hit.line), index));
        }

        if rows.is_empty() {
            self.nav.insert().text(fl!("no-matches"));
            return;
        }
        for (heading, text, index) in rows {
            if let Some(heading) = heading {
                self.nav.insert().text(heading);
            }
            self.nav.insert().text(text).data(NavTarget::Result(index));
        }
        if results.truncated {
            self.nav
                .insert()
                .text(fl!("results-truncated", total = project::SEARCH_LIMIT));
        }
    }

    /// The tab that already holds the file at `path`, if one does.
    fn tab_for(&self, path: &std::path::Path) -> Option<segmented_button::Entity> {
        self.tabs.iter().find(|id| {
            self.tabs
                .data::<Document>(*id)
                .and_then(|d| d.path.as_deref())
                == Some(path)
        })
    }

    /// Opens a file.
    fn load(&mut self, path: PathBuf, format: Format, source: &str) {
        let node = self.converters.parse(format, source);
        let document = Document::over(&self.schema, node, Some(path), format);
        self.error = None;
        self.place(document);
    }

    /// Shows a document: in the tab that is showing when that one is empty
    /// and untouched — a new window should not leave a blank tab behind — and
    /// in a tab of its own otherwise.
    fn place(&mut self, document: Document) {
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

    /// Selects the `ordinal`th match of the find bar's query in the active
    /// document.
    ///
    /// The folder search counted matches in the file's *source*; this counts
    /// them in the document that source parsed into. For plain text the two
    /// agree exactly. For Markdown a query that matches syntax the document
    /// does not carry — `**`, say — will not line up, and landing on the
    /// first match is the right answer then.
    fn land_on(&mut self, ordinal: usize) {
        let Some(find) = &self.find else {
            return;
        };
        let Ok(query) = Query::new(&find.query, find.matching) else {
            return;
        };
        let hits = search::find(self.doc().state.doc(), &query);
        let Some(hit) = hits.get(ordinal).or_else(|| hits.first()).copied() else {
            return;
        };
        let index = hits.iter().position(|h| *h == hit);
        if let Some(find) = &mut self.find {
            find.hits = hits;
            find.current = index;
        }
        let tr = search::select(&self.doc().state, hit);
        self.apply(tr);
    }

    /// Writes the active document.
    ///
    /// The tab and the document as written travel with the result, because
    /// the write is asynchronous: by the time it lands the user may have
    /// switched tabs or typed more, and neither should be marked saved.
    fn save_to(&self, path: PathBuf, format: Format) -> Task<Message> {
        let tab = self.tabs.active();
        let written = self.doc().state.doc().clone();
        let contents = self.converters.write(format, &written);
        cosmic::task::future(async move {
            match crate::document::write(path, contents).await {
                Ok(path) => Message::Saved(tab, path, format, written),
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

impl App {
    /// Opens what a launch named: a folder, a file, or a file that is not
    /// there yet.
    fn open_given(&mut self, path: PathBuf) -> Task<Message> {
        // A folder rather than a file: `magnetar-pencil ~/notes` opens the
        // folder, which is also how a new window inherits the one it came
        // from.
        if path.is_dir() {
            return cosmic::task::message(Message::FolderOpened(path));
        }
        if path.exists() {
            return open_path(path);
        }
        // A file that is not there yet, the way `vim notes.md` starts one: an
        // empty document already named, written there on the first Save.
        if let Some(tab) = self.tab_for(&path) {
            return self.update(Message::TabActivate(tab));
        }
        let mut document = Document::empty(&self.schema);
        document.format = Format::of(&path);
        document.path = Some(path);
        self.place(document);
        Task::none()
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

impl cosmic::Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
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
            persisted: config.clone(),
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
            results: None,
            jump: None,
            find: None,
            nav: nav_bar::Model::default(),
            error: None,
            context: None,
            pending: None,
            after_save: None,
            mode: nib_model::vim::Mode::Normal,
        };
        app.reload_speller();
        app.refresh();

        let mut tasks = vec![cosmic::command::set_theme(app.config.appearance().theme())];
        for path in flags.paths() {
            tasks.push(app.open_given(path));
        }
        (app, Task::batch(tasks))
    }

    /// A second launch handing over what it was asked to open.
    ///
    /// The window has already been raised by the time this is called; what is
    /// left is to open the files, each in a tab, exactly as the first launch
    /// would have.
    fn dbus_activation(&mut self, message: cosmic::dbus_activation::Message) -> Task<Message> {
        use cosmic::dbus_activation::Details;

        let paths = match message.msg {
            Details::ActivateAction { action, args } if action == launch::OPEN => {
                launch::paths(&args)
            }
            // A launcher that speaks the interface itself sends the files as
            // URLs rather than through a second `magnetar-pencil`.
            Details::Open { url } => url
                .iter()
                .filter_map(|url| url.to_file_path().ok())
                .collect(),
            // Nothing to open, or an action Pencil does not have: raising the
            // window was the whole request.
            Details::Activate | Details::ActivateAction { .. } => Vec::new(),
        };
        let mut tasks = Vec::with_capacity(paths.len());
        for path in paths {
            tasks.push(self.open_given(path));
        }
        Task::batch(tasks)
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
            button("window-new-symbolic", fl!("new-window"), Message::NewWindow),
            button("document-open-symbolic", fl!("open"), Message::Open),
            button("document-print-symbolic", fl!("print"), Message::Print),
            button("document-save-symbolic", fl!("save"), Message::Save),
            button("document-save-as-symbolic", fl!("save-as"), Message::SaveAs),
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
        let mut buttons = vec![button(
            "folder-open-symbolic",
            fl!("open-folder"),
            Message::OpenFolder,
        )];
        if self.project.is_some() {
            buttons.push(button(
                "list-remove-symbolic",
                fl!("close-folder"),
                Message::CloseFolder,
            ));
        }
        buttons.extend([
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
            button("view-list-symbolic", fl!("outline"), Message::ToggleOutline),
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
        ]);
        buttons
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        self.config.outline.then_some(&self.nav)
    }

    /// The header bar's close button.
    ///
    /// libcosmic closes the window unless this returns a message, so it always
    /// does: [`Message::CloseRequested`] exits straight away when nothing is
    /// unsaved.
    fn on_app_exit(&mut self) -> Option<Message> {
        Some(Message::CloseRequested)
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
                // A file already open is switched to rather than read again.
                match self.tab_for(&path) {
                    Some(id) => self.update(Message::TabActivate(id)),
                    None => open_path(path),
                }
            }
            Some(NavTarget::Result(index)) => self.update(Message::GoToResult(index)),
            None => Task::none(),
        }
    }

    fn context_drawer(&self) -> Option<cosmic::app::context_drawer::ContextDrawer<'_, Message>> {
        let page = self.context?;
        Some(
            cosmic::app::context_drawer::context_drawer(
                match page {
                    ContextPage::Settings => self.settings_page(),
                    ContextPage::Shortcuts => self.shortcuts_page(),
                    ContextPage::Statistics => self.statistics_page(),
                    ContextPage::Recent => self.recent_page(),
                    ContextPage::About => Self::about_page(),
                },
                Message::CloseContext,
            )
            .title(page.title()),
        )
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let pending = self.pending.as_ref()?;
        // Closing the window says how many more questions are coming.
        let body = match pending {
            Pending::Quit(discarded) => {
                let active = self.tabs.active();
                let others = self
                    .tabs
                    .iter()
                    .filter(|id| {
                        *id != active
                            && !discarded.contains(id)
                            && self
                                .tabs
                                .data::<Document>(*id)
                                .is_some_and(|document| document.dirty)
                    })
                    .count();
                if others == 0 {
                    fl!("unsaved-body")
                } else {
                    format!(
                        "{}\n\n{}",
                        fl!("unsaved-body"),
                        fl!("unsaved-others", count = others)
                    )
                }
            }
            Pending::CloseTab(_) => fl!("unsaved-body"),
        };
        Some(
            widget::dialog()
                .title(fl!("unsaved-title", name = self.doc().title()))
                .body(body)
                .primary_action(
                    widget::button::suggested(fl!("save-changes")).on_press(Message::ConfirmSave),
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
            // The desktop asking the window to close (a keyboard shortcut, the
            // window menu). `exit_on_close(false)` in `main` turns that into a
            // request rather than a close.
            cosmic::iced::window::close_requests().map(|_| Message::CloseRequested),
            // Zoom and the file shortcuts are the application's, not the
            // document's: the editor's keymap produces transactions, and none
            // of these is one.
            cosmic::iced::event::listen_with(|event, _status, _window| {
                let cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                    key,
                    modifiers,
                    ..
                }) = event
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
                    "p" => Some(Message::Print),
                    "n" if modifiers.shift() => Some(Message::NewWindow),
                    "n" => Some(Message::New),
                    "f" if modifiers.shift() => Some(Message::FindInProject),
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
                        return cosmic::iced::clipboard::write(text).map(cosmic::Action::App);
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
            Message::Edit(Action::Mode(mode)) => self.mode = mode,
            // Vim's `/`, `?` and `:` are prompts the widget does not draw, and
            // `n` and `N` step through what the last one found.
            Message::Edit(Action::Prompt(what)) => {
                return match what {
                    'n' => self.update(Message::FindNext),
                    'N' => self.update(Message::FindPrevious),
                    _ if self.find.is_some() => {
                        widget::text_input::focus(crate::chrome::find_input())
                    }
                    _ => self.update(Message::ToggleFind),
                };
            }
            // Everything else the widget reports needs nothing from here. A
            // right-click is the notable one: the widget has already moved the
            // caret, and the menu opens itself on the button's release.
            // ...and a task whose work was the point reports nothing either.
            Message::Edit(_) | Message::Ignore => {}

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
                    let dialog =
                        cosmic::dialog::file_chooser::open::Dialog::new().title(fl!("open"));
                    match dialog.open_file().await {
                        Ok(response) => match response.url().to_file_path() {
                            Ok(path) => match crate::document::read(path).await {
                                Ok((path, format, source)) => Message::Opened(path, format, source),
                                Err(error) => Message::Failed(error.to_string()),
                            },
                            Err(()) => Message::Failed(fl!("not-a-file")),
                        },
                        // A cancelled dialog is not an error; it is an answer.
                        Err(_) => Message::DismissError,
                    }
                });
            }
            Message::Print => {
                let doc = self.doc().state.doc().clone();
                let title = self.doc().title();
                return cosmic::task::future(async move {
                    // Laying a document out is the same work the window does
                    // to draw one, so it happens off the thread that draws.
                    let name = title.clone();
                    let pdf = tokio::task::spawn_blocking(move || {
                        crate::print::to_pdf(&doc, &name, crate::print::Sheet::default())
                    })
                    .await;
                    match pdf {
                        Ok(pdf) => match crate::print::send(pdf, &title).await {
                            Ok(()) => Message::Ignore,
                            Err(error) => Message::Failed(error),
                        },
                        Err(error) => Message::Failed(error.to_string()),
                    }
                });
            }
            Message::NewWindow => {
                // A process, not a window. libcosmic draws the header bar,
                // the nav bar and the context drawer for the *main* window
                // only, so a second window inside this process would come up
                // without any of the chrome that makes it an application.
                // A second process is a real second window, with its own
                // documents, its own undo and its own everything.
                let exe = match std::env::current_exe() {
                    Ok(exe) => exe,
                    Err(error) => {
                        self.error = Some(error.to_string());
                        return Task::none();
                    }
                };
                // The new window starts where this one is: same folder, empty
                // document.
                let folder = self.project.as_ref().map(|p| p.root().to_path_buf());
                return cosmic::task::future(async move {
                    let mut command = tokio::process::Command::new(exe);
                    // Said out loud: without it the new process would hand
                    // its arguments to this window and exit.
                    command.arg(launch::NEW_WINDOW);
                    if let Some(folder) = folder {
                        command.arg(folder);
                    }
                    match command.spawn() {
                        // Awaited so the child is reaped rather than left a
                        // zombie for as long as this window lives.
                        Ok(mut child) => {
                            let _ = child.wait().await;
                            Message::Ignore
                        }
                        Err(error) => Message::Failed(error.to_string()),
                    }
                });
            }
            Message::Opened(path, format, source) => {
                self.remember(&path);
                let jump = self
                    .jump
                    .take()
                    .filter(|(waiting, _)| waiting == &path)
                    .map(|(_, ordinal)| ordinal);
                // A file already open is switched to, not opened a second
                // time: two tabs on one file save over each other. The open
                // tab keeps its unsaved edits rather than being re-read.
                match self.tab_for(&path) {
                    Some(id) => {
                        let _ = self.update(Message::TabActivate(id));
                    }
                    None => self.load(path, format, &source),
                }
                if let Some(ordinal) = jump {
                    self.land_on(ordinal);
                }
            }

            Message::ConfirmCancel => self.pending = None,
            Message::ConfirmSave => {
                // Save, and let the save's own completion carry on with what
                // was waiting.
                let tab = self.tabs.active();
                self.after_save = self.pending.take().map(|pending| (tab, pending));
                return self.update(Message::Save);
            }
            Message::ConfirmDiscard => {
                let Some(pending) = self.pending.take() else {
                    return Task::none();
                };
                // Closing the window remembers the choice instead of marking
                // the tab clean: a Cancel on a later tab keeps this one's
                // changes, and they must still count as unsaved.
                if let Pending::Quit(mut discarded) = pending {
                    discarded.push(self.tabs.active());
                    return self.quit(discarded);
                }
                self.doc_mut().dirty = false;
                return self.resume(pending);
            }
            Message::CloseRequested => return self.quit(Vec::new()),

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
                        Err(_) => Message::SaveCancelled,
                    }
                });
            }
            // The document takes its new path and format when the write has
            // succeeded, not before: a failed Save As leaves it where it was.
            Message::SaveTo(path, format) => return self.save_to(path, format),
            Message::Saved(tab, path, format, written) => {
                self.remember(&path);
                if let Some(document) = self.tabs.data_mut::<Document>(tab) {
                    document.path = Some(path);
                    document.format = format;
                    // Typing that happened while the write was in flight is
                    // not on disk, so it stays unsaved.
                    document.dirty = document.state.doc() != &written;
                }
                self.error = None;
                self.retitle_tab(tab);
                if let Some(project) = &mut self.project {
                    project.refresh_status();
                }
                self.rebuild_nav();
                // A save that something was waiting on carries on with it —
                // only if it was that tab's save, and it left nothing unsaved.
                let clean = self
                    .tabs
                    .data::<Document>(tab)
                    .is_some_and(|document| !document.dirty);
                if self
                    .after_save
                    .as_ref()
                    .is_some_and(|(waiting, _)| *waiting == tab)
                {
                    let (_, pending) = self.after_save.take().expect("just checked");
                    if clean {
                        return self.resume(pending);
                    }
                }
            }
            // A save that did not happen releases whatever was waiting on it;
            // carrying on later, after some other save, would discard work.
            Message::SaveCancelled => self.after_save = None,
            Message::Failed(message) => {
                self.after_save = None;
                self.error = Some(message);
            }
            Message::DismissError => self.error = None,

            Message::ToggleFind => {
                self.find = match self.find {
                    Some(_) => None,
                    None => Some(Find::default()),
                };
                self.refresh();
                // Opening the bar puts the caret in it. Anything else and the
                // next thing typed lands in the document.
                if self.find.is_some() {
                    return widget::text_input::focus(crate::chrome::find_input());
                }
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
                            find.current =
                                search::next_from(&find.hits, doc.state.selection().from());
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
                if let Some(tr) = search::replace_all(&self.doc().state, &query, &find.replacement)
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
                self.config.text_size = size.clamp(
                    crate::config::MINIMUM_TEXT_SIZE,
                    crate::config::MAXIMUM_TEXT_SIZE,
                );
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
            // Another window changed the settings. What was built from them
            // is rebuilt: the dictionary (spell checking, its language, a
            // word learnt there) and the theme.
            Message::ConfigChanged(config) => {
                let speller = self.config.spell_check != config.spell_check
                    || self.config.spell_language != config.spell_language
                    || self.config.learnt_words != config.learnt_words;
                let theme = self.config.appearance != config.appearance;
                self.persisted = config.clone();
                self.config = config;
                if speller {
                    self.reload_speller();
                    self.refresh();
                }
                if theme {
                    return cosmic::command::set_theme(self.config.appearance().theme());
                }
            }

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
                    let dialog =
                        cosmic::dialog::file_chooser::open::Dialog::new().title(fl!("open-folder"));
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

            Message::FindInProject => {
                let Some(root) = self.project.as_ref().map(|p| p.root().to_path_buf()) else {
                    self.error = Some(fl!("no-folder-open"));
                    return Task::none();
                };
                // Nothing to search for yet: open the bar and wait for one.
                if self.find.as_ref().is_none_or(|find| find.query.is_empty()) {
                    if self.find.is_none() {
                        return self.update(Message::ToggleFind);
                    }
                    return widget::text_input::focus(crate::chrome::find_input());
                }
                let find = self.find.as_ref().expect("just checked");
                let (query, matching) = (find.query.clone(), find.matching);
                let compiled = match Query::new(&query, matching) {
                    Ok(compiled) => compiled,
                    Err(error) => {
                        if let Some(find) = &mut self.find {
                            find.error = Some(error.to_string());
                        }
                        return Task::none();
                    }
                };
                self.results = Some(Results {
                    query,
                    matching,
                    hits: Vec::new(),
                    truncated: false,
                    running: true,
                });
                self.sidebar = Sidebar::Results;
                self.config.outline = true;
                self.rebuild_nav();
                // Off the UI thread: a folder of any size is a lot of file
                // reads, and a frozen window is not a search.
                return cosmic::task::future(async move {
                    let hits = tokio::task::spawn_blocking(move || {
                        crate::project::search(&root, &compiled)
                    })
                    .await
                    .unwrap_or_default();
                    Message::ProjectSearched(hits)
                });
            }
            Message::ProjectSearched(hits) => {
                if let Some(results) = &mut self.results {
                    results.truncated = hits.len() >= project::SEARCH_LIMIT;
                    results.hits = hits;
                    results.running = false;
                }
                self.rebuild_nav();
            }
            Message::GoToResult(index) => {
                let Some(results) = &self.results else {
                    return Task::none();
                };
                let Some(hit) = results.hits.get(index) else {
                    return Task::none();
                };
                let (path, ordinal) = (hit.path.clone(), hit.ordinal);
                // The find bar takes the folder search's query, so the
                // document that opens is searched for the same thing.
                let (query, matching) = (results.query.clone(), results.matching);
                self.find = Some(Find {
                    query,
                    matching,
                    ..Find::default()
                });

                let Some(id) = self.tab_for(&path) else {
                    self.jump = Some((path.clone(), ordinal));
                    return open_path(path);
                };
                let task = self.update(Message::TabActivate(id));
                self.land_on(ordinal);
                return task;
            }

            Message::SetLineNumbers(on) => {
                self.config.line_numbers = on;
                self.write_config();
            }
            Message::SetWrapCode(on) => {
                self.config.wrap_code = on;
                self.write_config();
            }
            Message::SetVim(on) => {
                self.config.vim = on;
                self.mode = nib_model::vim::Mode::Normal;
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
            .vim(self.config.vim)
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
            screen = screen.push(Self::find_bar(find, self.project.is_some()));
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

    pub(crate) fn vim_enabled(&self) -> bool {
        self.config.vim
    }

    /// What the status bar shows: the mode, and any half-typed command.
    pub(crate) fn mode_label(&self) -> String {
        self.mode.label().to_owned()
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
        let Some(handler) = &self.config_handler else {
            return;
        };
        match self.config.write_changed(&self.persisted, handler) {
            Ok(()) => self.persisted = self.config.clone(),
            // A setting that will not persist is worth saying once, in the
            // place errors go, rather than swallowing.
            Err(error) => self.error = Some(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An application with one empty tab, writing no settings: a test must
    /// not rewrite the user's recent-files list.
    fn app() -> App {
        let (mut app, _) = <App as cosmic::Application>::init(Core::default(), Flags::default());
        app.config_handler = None;
        app
    }

    /// Marks the active document as edited and gives it a path.
    fn edited(app: &mut App, path: Option<&str>) {
        app.doc_mut().path = path.map(PathBuf::from);
        app.doc_mut().dirty = true;
    }

    /// A save finishes after the user has moved to another tab. What it
    /// wrote was the first tab, so the first tab is the one it cleans.
    #[test]
    fn a_save_marks_clean_the_tab_it_saved_not_the_one_now_showing() {
        let mut app = app();
        edited(&mut app, Some("/tmp/pencil-a.md"));
        let saving = app.tabs.active();

        app.add_tab(Document::empty(&app.schema.clone()));
        edited(&mut app, None);
        let showing = app.tabs.active();
        assert_ne!(saving, showing);

        let _ = app.update(Message::Saved(
            saving,
            PathBuf::from("/tmp/pencil-a.md"),
            Format::Markdown,
            written(&app, saving),
        ));

        let shown = app.tabs.data::<Document>(showing).unwrap();
        assert!(shown.dirty, "the unsaved tab was marked saved");
        assert_eq!(
            shown.path, None,
            "the unsaved tab took the other file's path"
        );
        assert!(!app.tabs.data::<Document>(saving).unwrap().dirty);
    }

    /// Typing that lands while a write is in flight is not in the file.
    #[test]
    fn edits_made_while_a_save_is_in_flight_stay_unsaved() {
        let mut app = app();
        edited(&mut app, Some("/tmp/pencil-c.md"));
        let tab = app.tabs.active();
        let on_disk = written(&app, tab);

        let mut tr = app.doc().state.tr();
        tr.insert_text("more").expect("text goes in at the caret");
        App::apply(&mut app, tr);

        let _ = app.update(Message::Saved(
            tab,
            PathBuf::from("/tmp/pencil-c.md"),
            Format::Markdown,
            on_disk,
        ));
        assert!(
            app.doc().dirty,
            "text typed during the save was marked saved"
        );
    }

    /// Save As names the document only once the write has succeeded; a
    /// failed one leaves its name and format where they were.
    #[test]
    fn a_save_as_that_fails_leaves_the_name_and_format_alone() {
        let mut app = app();
        edited(&mut app, Some("/tmp/pencil-d.md"));
        let _ = app.update(Message::SaveTo(
            PathBuf::from("/nonesuch/pencil-d.html"),
            Format::Html,
        ));
        let _ = app.update(Message::Failed("no such folder".to_owned()));
        assert_eq!(
            app.doc().path.as_deref(),
            Some(std::path::Path::new("/tmp/pencil-d.md"))
        );
        assert_eq!(app.doc().format, Format::Markdown);
        assert!(app.doc().dirty);
    }

    /// The document of a tab, as a save would have written it.
    fn written(app: &App, tab: segmented_button::Entity) -> Node {
        app.tabs.data::<Document>(tab).unwrap().state.doc().clone()
    }

    /// Save-before-close whose dialog is cancelled must not leave the close
    /// armed: the next unrelated save would carry it out, discarding the tab.
    #[test]
    fn a_cancelled_save_before_closing_does_not_close_the_tab_later() {
        let mut app = app();
        edited(&mut app, None);
        let untitled = app.tabs.active();
        app.add_tab(Document::empty(&app.schema.clone()));
        edited(&mut app, Some("/tmp/pencil-b.md"));
        let other = app.tabs.active();

        let _ = app.update(Message::TabClose(untitled));
        let _ = app.update(Message::ConfirmSave);
        // The Save As dialog for the untitled tab is dismissed.
        let _ = app.update(Message::SaveCancelled);

        let _ = app.update(Message::TabActivate(other));
        let _ = app.update(Message::Saved(
            other,
            PathBuf::from("/tmp/pencil-b.md"),
            Format::Markdown,
            written(&app, other),
        ));

        assert!(
            app.tabs.data::<Document>(untitled).is_some(),
            "the untitled tab was closed by an unrelated save"
        );
    }

    /// Closing the window asks about each unsaved tab in turn. A Discard
    /// moves on to the next; a Cancel keeps everything, and a tab discarded
    /// earlier in that round still counts as unsaved.
    #[test]
    fn closing_the_window_asks_about_each_unsaved_tab() {
        let mut app = app();
        edited(&mut app, Some("/tmp/pencil-e.md"));
        let first = app.tabs.active();
        app.add_tab(Document::empty(&app.schema.clone()));
        let clean = app.tabs.active();
        app.add_tab(Document::empty(&app.schema.clone()));
        edited(&mut app, None);
        let second = app.tabs.active();

        let _ = app.update(Message::CloseRequested);
        assert_eq!(app.pending, Some(Pending::Quit(Vec::new())));
        assert_eq!(
            app.tabs.active(),
            first,
            "the first unsaved tab is asked about"
        );

        let _ = app.update(Message::ConfirmDiscard);
        assert_eq!(app.pending, Some(Pending::Quit(vec![first])));
        assert_eq!(app.tabs.active(), second, "then the next one");

        let _ = app.update(Message::ConfirmCancel);
        assert_eq!(app.pending, None);
        assert_eq!(app.tabs.iter().count(), 3, "a cancelled close closed tabs");
        assert!(
            app.tabs.data::<Document>(first).unwrap().dirty,
            "a tab discarded before the Cancel was marked saved"
        );
        assert!(!app.tabs.data::<Document>(clean).unwrap().dirty);
    }

    /// Saving from the close dialog moves on to the next unsaved tab once the
    /// save has landed.
    #[test]
    fn a_save_while_closing_the_window_moves_on_to_the_next_tab() {
        let mut app = app();
        edited(&mut app, Some("/tmp/pencil-f.md"));
        let first = app.tabs.active();
        app.add_tab(Document::empty(&app.schema.clone()));
        edited(&mut app, None);
        let second = app.tabs.active();

        let _ = app.update(Message::CloseRequested);
        let _ = app.update(Message::ConfirmSave);
        assert_eq!(app.pending, None, "the question was answered");
        let _ = app.update(Message::Saved(
            first,
            PathBuf::from("/tmp/pencil-f.md"),
            Format::Markdown,
            written(&app, first),
        ));

        assert_eq!(app.pending, Some(Pending::Quit(Vec::new())));
        assert_eq!(app.tabs.active(), second);
    }

    /// Nothing unsaved: the window closes without a question.
    #[test]
    fn closing_the_window_with_nothing_unsaved_asks_nothing() {
        let mut app = app();
        app.add_tab(Document::empty(&app.schema.clone()));
        let _ = app.update(Message::CloseRequested);
        assert_eq!(app.pending, None);
    }

    /// The header bar's close button goes through the same question: left
    /// to libcosmic, it closes the window outright.
    #[test]
    fn the_close_button_is_a_request_the_app_answers() {
        let mut app = app();
        assert!(matches!(
            cosmic::Application::on_app_exit(&mut app),
            Some(Message::CloseRequested)
        ));
    }

    /// Opening a file that is already open switches to its tab: two tabs on
    /// one file save over each other.
    #[test]
    fn opening_a_file_already_open_switches_to_its_tab() {
        let mut app = app();
        let a = PathBuf::from("/tmp/pencil-g.md");
        let b = PathBuf::from("/tmp/pencil-h.md");
        let _ = app.update(Message::Opened(
            a.clone(),
            Format::Markdown,
            "# A\n".to_owned(),
        ));
        let first = app.tabs.active();
        let _ = app.update(Message::Opened(b, Format::Markdown, "# B\n".to_owned()));
        edited(&mut app, Some("/tmp/pencil-h.md"));

        let _ = app.update(Message::Opened(a, Format::Markdown, "# A\n".to_owned()));
        assert_eq!(app.tabs.iter().count(), 2, "the file was opened twice");
        assert_eq!(app.tabs.active(), first);
    }

    /// Spell checking turned off in another window turns off here too.
    #[test]
    fn a_settings_change_from_another_window_reaches_the_speller() {
        let mut app = app();
        app.config.spell_check = true;
        app.config.spell_language = "en_US".to_owned();
        app.reload_speller();
        if app.speller.is_none() {
            // No en_US dictionary installed: nothing to turn off.
            return;
        }

        let mut elsewhere = app.config.clone();
        elsewhere.spell_check = false;
        let _ = app.update(Message::ConfigChanged(elsewhere));
        assert!(app.speller.is_none(), "the speller outlived the setting");
    }

    /// An open folder can be closed: the header offers it once there is one.
    #[test]
    fn an_open_folder_has_a_close_button() {
        let mut app = app();
        let without = cosmic::Application::header_end(&app).len();
        app.project = Some(Project::open(std::env::temp_dir()));
        assert_eq!(cosmic::Application::header_end(&app).len(), without + 1);
        let _ = app.update(Message::CloseFolder);
        assert!(app.project.is_none());
    }

    /// What a second `magnetar-pencil <paths>` sends the running window.
    fn handed_over(paths: &[&std::path::Path]) -> cosmic::dbus_activation::Message {
        use cosmic::app::CosmicFlags as _;

        let flags = Flags::new(paths.iter().map(|path| path.to_path_buf()));
        cosmic::dbus_activation::Message {
            activation_token: None,
            desktop_startup_id: None,
            msg: cosmic::dbus_activation::Details::ActivateAction {
                action: flags.action().expect("there is something to open").clone(),
                args: flags.args().into_iter().map(str::to_owned).collect(),
            },
        }
    }

    /// A file opened while Pencil is running — a second launch handing its
    /// command line over — opens as a tab in the window that is up, and a
    /// file that is already open there is switched to.
    #[test]
    fn a_second_launch_opens_its_files_as_tabs_in_this_window() {
        let mut app = app();
        edited(&mut app, Some("/tmp/pencil-already-open.md"));
        let first = app.tabs.active();

        let a = std::env::temp_dir().join("pencil-handed-over-a.md");
        let b = std::env::temp_dir().join("pencil-handed-over-b.html");
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
        let _ = app.dbus_activation(handed_over(&[&a, &b]));

        assert_eq!(app.tabs.iter().count(), 3, "each file gets a tab");
        assert_eq!(app.doc().path.as_deref(), Some(b.as_path()));
        assert_eq!(app.doc().format, Format::Html);
        assert!(app.tab_for(&a).is_some());
        assert!(app.tabs.data::<Document>(first).unwrap().dirty);

        let _ = app.dbus_activation(handed_over(&[&a]));
        assert_eq!(app.tabs.iter().count(), 3, "the file was opened twice");
        assert_eq!(app.doc().path.as_deref(), Some(a.as_path()));
    }

    /// A launch with nothing to open raises the window and changes nothing
    /// in it, and an action Pencil does not have is not mistaken for a file.
    #[test]
    fn a_second_launch_with_nothing_to_open_opens_nothing() {
        use cosmic::dbus_activation::{Details, Message as Activation};

        let mut app = app();
        for msg in [
            Details::Activate,
            Details::ActivateAction {
                action: "print".to_owned(),
                args: vec!["file:///tmp/pencil-not-this.md".to_owned()],
            },
        ] {
            let _ = app.dbus_activation(Activation {
                activation_token: None,
                desktop_startup_id: None,
                msg,
            });
        }
        assert_eq!(app.tabs.iter().count(), 1);
        assert_eq!(app.doc().path, None);
    }

    /// A launcher that speaks the activation interface itself sends URLs.
    #[test]
    fn files_sent_as_urls_open_as_tabs_too() {
        let mut app = app();
        let path = std::env::temp_dir().join("pencil-sent-as-url.md");
        let _ = std::fs::remove_file(&path);
        let _ = app.dbus_activation(cosmic::dbus_activation::Message {
            activation_token: None,
            desktop_startup_id: None,
            msg: cosmic::dbus_activation::Details::Open {
                url: vec![url::Url::from_file_path(&path).unwrap()],
            },
        });
        assert_eq!(app.doc().path.as_deref(), Some(path.as_path()));
    }

    /// `magnetar-pencil new.md` for a file that does not exist yet starts a document
    /// by that name, instead of an untitled one.
    #[test]
    fn a_file_named_on_the_command_line_that_is_not_there_yet_is_started() {
        let path = std::env::temp_dir().join("pencil-not-there-yet.html");
        let _ = std::fs::remove_file(&path);
        let (mut app, _) =
            <App as cosmic::Application>::init(Core::default(), Flags::new([path.clone()]));
        app.config_handler = None;
        assert_eq!(app.doc().path.as_deref(), Some(path.as_path()));
        assert_eq!(app.doc().format, Format::Html);
        assert!(!app.doc().dirty);
    }
}
