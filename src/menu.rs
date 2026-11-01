// SPDX-License-Identifier: GPL-3.0-only

//! The application's commands: the menu bar, and the keyboard shortcuts that
//! reach the same commands.
//!
//! One table drives three things — what a shortcut does, the accelerator the
//! menu prints beside an entry, and the shortcuts page — so a key and its
//! menu entry cannot drift apart. The editor's own keys (bold, undo, lists)
//! are not here: they live in the document's keymap and make transactions.

use std::collections::HashMap;

use cosmic::iced::keyboard::Key;
use cosmic::widget::menu::{self, KeyBind, key_bind::Modifier};

use crate::app::{ContextPage, Message, Sidebar};
use crate::fl;

/// Every command the menu bar offers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum MenuAction {
    New,
    NewWindow,
    Open,
    OpenFolder,
    CloseFolder,
    Recent,
    Save,
    SaveAs,
    Print,
    Find,
    FindInProject,
    ToggleSidebar,
    ShowHeadings,
    ShowFiles,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Statistics,
    Shortcuts,
    Settings,
    About,
}

impl MenuAction {
    /// The name the menu and the shortcuts page give it.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::New => fl!("new"),
            Self::NewWindow => fl!("new-window"),
            Self::Open => fl!("open"),
            Self::OpenFolder => fl!("open-folder"),
            Self::CloseFolder => fl!("close-folder"),
            Self::Recent => fl!("recent"),
            Self::Save => fl!("save"),
            Self::SaveAs => fl!("save-as"),
            Self::Print => fl!("print"),
            Self::Find => fl!("find"),
            Self::FindInProject => fl!("find-in-project"),
            Self::ToggleSidebar => fl!("show-sidebar"),
            Self::ShowHeadings => fl!("sidebar-headings"),
            Self::ShowFiles => fl!("project"),
            Self::ZoomIn => fl!("zoom-in"),
            Self::ZoomOut => fl!("zoom-out"),
            Self::ZoomReset => fl!("zoom-reset"),
            Self::Statistics => fl!("statistics"),
            Self::Shortcuts => fl!("shortcuts"),
            Self::Settings => fl!("settings"),
            Self::About => fl!("about"),
        }
    }
}

impl menu::action::MenuAction for MenuAction {
    type Message = Message;

    fn message(&self) -> Message {
        match self {
            Self::New => Message::New,
            Self::NewWindow => Message::NewWindow,
            Self::Open => Message::Open,
            Self::OpenFolder => Message::OpenFolder,
            Self::CloseFolder => Message::CloseFolder,
            Self::Recent => Message::OpenContext(ContextPage::Recent),
            Self::Save => Message::Save,
            Self::SaveAs => Message::SaveAs,
            Self::Print => Message::Print,
            Self::Find => Message::ToggleFind,
            Self::FindInProject => Message::FindInProject,
            Self::ToggleSidebar => Message::ToggleOutline,
            Self::ShowHeadings => Message::ShowSidebar(Sidebar::Outline),
            Self::ShowFiles => Message::ShowSidebar(Sidebar::Project),
            Self::ZoomIn => Message::ZoomIn,
            Self::ZoomOut => Message::ZoomOut,
            Self::ZoomReset => Message::ZoomReset,
            Self::Statistics => Message::OpenContext(ContextPage::Statistics),
            Self::Shortcuts => Message::OpenContext(ContextPage::Shortcuts),
            Self::Settings => Message::OpenContext(ContextPage::Settings),
            Self::About => Message::OpenContext(ContextPage::About),
        }
    }
}

/// The application's shortcuts, in the order the shortcuts page lists them.
///
/// Zoom in has two: `Ctrl` + `=` and `Ctrl` + `+`, which on most layouts is
/// the same key with Shift held. Only the first is listed.
#[must_use]
pub fn bindings() -> Vec<(KeyBind, MenuAction)> {
    use Modifier::{Ctrl, Shift};

    let bind = |modifiers: &[Modifier], key: &str, action| {
        (
            KeyBind {
                modifiers: modifiers.to_vec(),
                key: Key::Character(key.into()),
            },
            action,
        )
    };
    vec![
        bind(&[Ctrl], "n", MenuAction::New),
        bind(&[Ctrl, Shift], "n", MenuAction::NewWindow),
        bind(&[Ctrl], "o", MenuAction::Open),
        bind(&[Ctrl], "s", MenuAction::Save),
        bind(&[Ctrl, Shift], "s", MenuAction::SaveAs),
        bind(&[Ctrl], "p", MenuAction::Print),
        bind(&[Ctrl], "f", MenuAction::Find),
        bind(&[Ctrl, Shift], "f", MenuAction::FindInProject),
        bind(&[Ctrl], "=", MenuAction::ZoomIn),
        bind(&[Ctrl, Shift], "+", MenuAction::ZoomIn),
        bind(&[Ctrl], "-", MenuAction::ZoomOut),
        bind(&[Ctrl], "0", MenuAction::ZoomReset),
    ]
}

/// [`bindings`] as libcosmic's menu takes them.
#[must_use]
pub fn key_binds() -> HashMap<KeyBind, MenuAction> {
    bindings().into_iter().collect()
}

/// The menu bar: File, Edit and View.
///
/// `folder_open` decides whether Close folder is offered and whether the
/// folder search is available; `sidebar` and `showing` mark the View menu's
/// sidebar entries.
/// The File menu. Close folder is there only while a folder is open.
fn file_menu(folder_open: bool) -> Vec<menu::Item<MenuAction, String>> {
    use menu::Item;

    let button = |action: MenuAction| Item::Button(action.label(), None, action);
    let mut file = vec![
        button(MenuAction::New),
        button(MenuAction::NewWindow),
        Item::Divider,
        button(MenuAction::Open),
        button(MenuAction::OpenFolder),
    ];
    if folder_open {
        file.push(button(MenuAction::CloseFolder));
    }
    file.extend([
        button(MenuAction::Recent),
        Item::Divider,
        button(MenuAction::Save),
        button(MenuAction::SaveAs),
        Item::Divider,
        button(MenuAction::Print),
    ]);
    file
}

// libcosmic's `menu::items` takes the map with the default hasher, so this
// cannot be generic over another.
#[allow(clippy::implicit_hasher)]
pub fn bar<'a>(
    key_binds: &HashMap<KeyBind, MenuAction>,
    folder_open: bool,
    sidebar: bool,
    showing: Sidebar,
) -> cosmic::Element<'a, Message> {
    use menu::Item;

    let label = |action: MenuAction| action.label();
    let button = |action: MenuAction| Item::Button(label(action), None, action);

    let file = file_menu(folder_open);

    let edit = vec![
        button(MenuAction::Find),
        // Greyed rather than hidden: an entry that appears when a folder
        // opens is an entry nobody knows is there.
        if folder_open {
            button(MenuAction::FindInProject)
        } else {
            Item::ButtonDisabled(
                label(MenuAction::FindInProject),
                None,
                MenuAction::FindInProject,
            )
        },
    ];

    let view = vec![
        Item::CheckBox(
            label(MenuAction::ToggleSidebar),
            None,
            sidebar,
            MenuAction::ToggleSidebar,
        ),
        Item::CheckBox(
            label(MenuAction::ShowHeadings),
            None,
            showing == Sidebar::Outline,
            MenuAction::ShowHeadings,
        ),
        Item::CheckBox(
            label(MenuAction::ShowFiles),
            None,
            showing == Sidebar::Project,
            MenuAction::ShowFiles,
        ),
        Item::Divider,
        button(MenuAction::ZoomIn),
        button(MenuAction::ZoomOut),
        button(MenuAction::ZoomReset),
        Item::Divider,
        button(MenuAction::Statistics),
        button(MenuAction::Shortcuts),
        button(MenuAction::Settings),
        button(MenuAction::About),
    ];

    let root = |name: String, items| {
        menu::Tree::with_children(
            cosmic::Element::from(menu::root(name)),
            menu::items(key_binds, items),
        )
    };
    // The width the suite's other menu bars use: the default 150 ellipsizes
    // labels and leaves the shortcut column no room.
    menu::bar(vec![
        root(fl!("menu-file"), file),
        root(fl!("menu-edit"), edit),
        root(fl!("menu-view"), view),
    ])
    .item_width(menu::ItemWidth::Uniform(260))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An open folder can be closed: the File menu offers it once there is
    /// one.
    #[test]
    fn close_folder_is_offered_only_with_a_folder_open() {
        let offers_close = |folder_open| {
            file_menu(folder_open)
                .iter()
                .any(|item| matches!(item, menu::Item::Button(_, _, MenuAction::CloseFolder)))
        };
        assert!(!offers_close(false));
        assert!(offers_close(true));
    }

    #[test]
    fn every_shortcut_is_one_key_for_one_command() {
        let all = bindings();
        assert_eq!(key_binds().len(), all.len(), "a key is bound twice");
    }
}
