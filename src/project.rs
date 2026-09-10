// SPDX-License-Identifier: GPL-3.0-only

//! A folder of documents, and what git says about them.
//!
//! # Why shelling out to git rather than linking a library
//!
//! Because the only thing this needs from git is the porcelain status of a
//! working tree, and `libgit2` is a C dependency that would have to be built,
//! packaged and kept current for a row of coloured dots. Running `git` means
//! no build dependency, exactly the same answer the user's own `git status`
//! would give — including their config, their ignore rules, their submodules —
//! and a failure mode that is simply "no dots" on a machine without git.
//!
//! # Why the tree is flat
//!
//! Same reason the document's blocks are: a nested model makes every question
//! about the thing the user is pointing at into a walk. Expanding a folder
//! splices its children into the list; collapsing removes them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What git says about a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Changed since the last commit, and not staged.
    Modified,
    /// Staged.
    Staged,
    /// Not tracked at all.
    Untracked,
    /// Tracked, unmodified. Not stored — the absence of an entry means this.
    Clean,
}

impl Status {
    /// The symbol shown beside the name.
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            Self::Modified => "\u{25cf}",
            Self::Staged => "\u{25c6}",
            Self::Untracked => "\u{25cb}",
            Self::Clean => "",
        }
    }
}

/// One row of the tree.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    /// How far in, for the indent.
    pub depth: usize,
    pub is_dir: bool,
    /// Whether a folder's children are spliced in below it.
    pub expanded: bool,
    pub status: Option<Status>,
}

impl Entry {
    #[must_use]
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_owned()
    }
}

/// An open folder.
#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
    entries: Vec<Entry>,
    status: BTreeMap<PathBuf, Status>,
}

/// Files Pencil can open, by extension.
///
/// A rich text editor that offered to open a binary would be offering to
/// mangle it.
const OPENABLE: &[&str] = &[
    "md", "markdown", "mdc", "mdx", "html", "htm", "xhtml", "txt", "text",
];

/// Folders that are never worth walking into.
const SKIP: &[&str] = &[".git", "target", "node_modules", ".venv", "__pycache__"];

impl Project {
    /// Opens a folder.
    #[must_use]
    pub fn open(root: PathBuf) -> Self {
        let status = git_status(&root);
        let mut project = Self {
            root,
            entries: Vec::new(),
            status,
        };
        project.entries = project.read(&project.root.clone(), 0);
        project
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Re-reads git's opinion, leaving the tree's shape alone.
    ///
    /// Called after a save: the file that changed is the one the user is
    /// looking at, and a dot that only appears on the next reopen is a dot
    /// nobody trusts.
    pub fn refresh_status(&mut self) {
        self.status = git_status(&self.root);
        for entry in &mut self.entries {
            entry.status = self.status.get(&entry.path).copied();
        }
    }

    /// Expands or collapses the folder at `index`.
    pub fn toggle(&mut self, index: usize) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        if !entry.is_dir {
            return;
        }
        let depth = entry.depth;
        let path = entry.path.clone();
        let expanded = entry.expanded;

        // Everything below it that is deeper belongs to it.
        let end = self.entries[index + 1..]
            .iter()
            .position(|e| e.depth <= depth)
            .map_or(self.entries.len(), |offset| index + 1 + offset);
        self.entries.drain(index + 1..end);

        if let Some(entry) = self.entries.get_mut(index) {
            entry.expanded = !expanded;
        }
        if !expanded {
            let children = self.read(&path, depth + 1);
            self.entries.splice(index + 1..=index, children);
        }
    }

    /// Reads one folder's worth of rows: folders first, then files, each in
    /// name order, and nothing this editor could not open.
    fn read(&self, path: &Path, depth: usize) -> Vec<Entry> {
        let Ok(dir) = std::fs::read_dir(path) else {
            return Vec::new();
        };
        let mut folders = Vec::new();
        let mut files = Vec::new();

        for entry in dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && name != ".." || SKIP.contains(&name.as_str()) {
                continue;
            }
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir {
                folders.push(Entry {
                    status: self.status.get(&path).copied(),
                    path,
                    depth,
                    is_dir: true,
                    expanded: false,
                });
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .is_some_and(|e| OPENABLE.contains(&e.as_str()))
            {
                files.push(Entry {
                    status: self.status.get(&path).copied(),
                    path,
                    depth,
                    is_dir: false,
                    expanded: false,
                });
            }
        }
        folders.sort_by_key(Entry::name);
        files.sort_by_key(Entry::name);
        folders.extend(files);
        folders
    }
}

/// What `git status` says about the working tree, by absolute path.
///
/// An empty map when the folder is not a repository, or git is not installed,
/// or it fails for any other reason — none of which is worth an error, because
/// the answer to all three is the same: no dots.
fn git_status(root: &Path) -> BTreeMap<PathBuf, Status> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "-z", "--untracked-files=normal"])
        .output()
    else {
        return BTreeMap::new();
    };
    if !output.status.success() {
        return BTreeMap::new();
    }

    let mut statuses = BTreeMap::new();
    // NUL-separated, each record `XY <path>`. NUL rather than newlines because
    // a filename may contain one and git will not quote it in this mode.
    for record in output.stdout.split(|b| *b == 0) {
        if record.len() < 4 {
            continue;
        }
        let Ok(record) = std::str::from_utf8(record) else {
            continue;
        };
        let (flags, path) = record.split_at(2);
        let path = root.join(path.trim_start());
        let status = match flags.as_bytes() {
            [b'?', b'?'] => Status::Untracked,
            [b' ', _] => Status::Modified,
            _ => Status::Staged,
        };
        statuses.insert(path.clone(), status);

        // A change anywhere marks every folder above it, so a collapsed tree
        // still says where to look.
        let mut parent = path.parent();
        while let Some(dir) = parent {
            if !dir.starts_with(root) || dir == root {
                break;
            }
            statuses.entry(dir.to_path_buf()).or_insert(Status::Modified);
            parent = dir.parent();
        }
    }
    statuses
}
