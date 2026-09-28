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

use nib_model::search::Query;

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
        // `all` rather than `normal`: the default collapses an untracked
        // folder to one entry, and a tree that marks the folder but not the
        // file in it is a tree that points at the wrong thing.
        .args(["status", "--porcelain", "-z", "--untracked-files=all"])
        .output()
    else {
        return BTreeMap::new();
    };
    if !output.status.success() {
        return BTreeMap::new();
    }

    parse_status(root, &output.stdout)
}

/// Reads `git status --porcelain -z` output into statuses by absolute path.
///
/// NUL-separated, each record `XY <path>`. NUL rather than newlines because a
/// filename may contain one and git will not quote it in this mode. A rename
/// or a copy (`R` or `C` in either column) is followed by one more record,
/// the path it came from, with no flags of its own; it is skipped rather than
/// read as `XY <path>`, which would take the first two bytes of a filename
/// for flags.
fn parse_status(root: &Path, output: &[u8]) -> BTreeMap<PathBuf, Status> {
    let mut statuses = BTreeMap::new();
    let mut records = output.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let (flags, path) = record.split_at(2);
        if flags.iter().any(|flag| matches!(flag, b'R' | b'C')) {
            records.next();
        }
        let Ok(path) = std::str::from_utf8(path) else {
            continue;
        };
        let path = root.join(path.trim_start());
        let status = match flags {
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
            statuses
                .entry(dir.to_path_buf())
                .or_insert(Status::Modified);
            parent = dir.parent();
        }
    }
    statuses
}

/// One match, somewhere under the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// Counting from one, the way an editor counts.
    pub line: usize,
    /// The whole line the match is on, trimmed for display.
    pub text: String,
    /// Which match this is within its own file, counting from zero.
    ///
    /// The document the file parses into is searched again when the file
    /// opens, and this says which of *those* hits to land on — the fifth match
    /// in the file is the fifth match in the document.
    pub ordinal: usize,
}

/// How many matches are worth collecting before stopping.
///
/// A sidebar nobody can scroll to the end of is a sidebar that only cost time
/// to build. The count is shown, so a truncated search says so.
pub const SEARCH_LIMIT: usize = 500;

/// Every match for `query` in the files under `root`.
///
/// Blocking, and meant to be run off the UI thread. Files that are not UTF-8
/// are skipped rather than reported: a folder of documents may well contain a
/// PNG, and that is not an error the user needs telling about.
#[must_use]
pub fn search(root: &Path, query: &Query) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut folders = vec![root.to_path_buf()];

    while let Some(folder) = folders.pop() {
        if hits.len() >= SEARCH_LIMIT {
            break;
        }
        let Ok(dir) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut files = Vec::new();
        for entry in dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                folders.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .is_some_and(|e| OPENABLE.contains(&e.as_str()))
            {
                files.push(path);
            }
        }
        // In name order, so the same folder searched twice reads the same.
        files.sort();
        for path in files {
            search_file(&path, query, &mut hits);
            if hits.len() >= SEARCH_LIMIT {
                break;
            }
        }
    }
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    hits
}

fn search_file(path: &Path, query: &Query, hits: &mut Vec<Hit>) {
    let Ok(source) = std::fs::read_to_string(path) else {
        return;
    };
    let mut ordinal = 0;
    for (index, line) in source.lines().enumerate() {
        for _ in query.matches(line) {
            hits.push(Hit {
                path: path.to_path_buf(),
                line: index + 1,
                text: line.trim().to_owned(),
                ordinal,
            });
            ordinal += 1;
            if hits.len() >= SEARCH_LIMIT {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rename is two records, and the second is the old path with no
    /// flags. Read as a record of its own, `old.md` became the flags `ol` on
    /// a file called `d.md`.
    #[test]
    fn a_rename_marks_the_new_path_and_nothing_made_of_the_old_one() {
        let root = Path::new("/project");
        let statuses = parse_status(root, b"R  notes/new.md\0notes/old.md\0 M other.md\0");

        assert_eq!(
            statuses.get(Path::new("/project/notes/new.md")),
            Some(&Status::Staged)
        );
        assert_eq!(
            statuses.get(Path::new("/project/other.md")),
            Some(&Status::Modified)
        );
        assert_eq!(
            statuses.keys().collect::<Vec<_>>(),
            [
                Path::new("/project/notes"),
                Path::new("/project/notes/new.md"),
                Path::new("/project/other.md"),
            ],
            "a path was made out of the rename's old name"
        );
    }
}
