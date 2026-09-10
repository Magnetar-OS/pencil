// SPDX-License-Identifier: GPL-3.0-only

//! The project tree: what it shows, what it hides, and what it says about git.

use std::fs;
use std::path::PathBuf;

use nib_model::search::{Matching, Query};
use pencil::project::{self, Project, Status};

/// A temporary folder with a few things in it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pencil-project-{name}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("make the scratch folder");
        Self(path)
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("make the parent");
        }
        fs::write(&path, contents).expect("write the file");
        path
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).expect("make the folder");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_project_lists_folders_before_files_each_in_name_order() {
    let scratch = Scratch::new("order");
    scratch.file("zebra.md", "z");
    scratch.file("apple.md", "a");
    scratch.dir("notes");
    scratch.dir("archive");

    let project = Project::open(scratch.0.clone());
    let names: Vec<String> = project.entries().iter().map(pencil::project::Entry::name).collect();
    assert_eq!(names, ["archive", "notes", "apple.md", "zebra.md"]);
}

#[test]
fn only_what_pencil_can_open_is_listed() {
    let scratch = Scratch::new("openable");
    scratch.file("notes.md", "");
    scratch.file("page.html", "");
    scratch.file("plain.txt", "");
    scratch.file("photo.png", "");
    scratch.file("program.rs", "");

    let project = Project::open(scratch.0.clone());
    let names: Vec<String> = project.entries().iter().map(pencil::project::Entry::name).collect();
    assert_eq!(
        names,
        ["notes.md", "page.html", "plain.txt"],
        "offering to open a binary is offering to mangle it"
    );
}

#[test]
fn hidden_and_noisy_folders_are_skipped() {
    let scratch = Scratch::new("skip");
    scratch.dir(".git");
    scratch.dir("target");
    scratch.dir("node_modules");
    scratch.dir("chapters");
    scratch.file(".hidden.md", "");

    let project = Project::open(scratch.0.clone());
    let names: Vec<String> = project.entries().iter().map(pencil::project::Entry::name).collect();
    assert_eq!(names, ["chapters"]);
}

#[test]
fn expanding_a_folder_splices_its_children_in_and_collapsing_removes_them() {
    let scratch = Scratch::new("expand");
    scratch.file("chapters/one.md", "");
    scratch.file("chapters/two.md", "");
    scratch.file("index.md", "");

    let mut project = Project::open(scratch.0.clone());
    assert_eq!(project.entries().len(), 2, "the folder and the file");

    project.toggle(0);
    let names: Vec<String> = project.entries().iter().map(pencil::project::Entry::name).collect();
    assert_eq!(names, ["chapters", "one.md", "two.md", "index.md"]);
    assert_eq!(project.entries()[1].depth, 1, "children are one level in");
    assert!(project.entries()[0].expanded);

    project.toggle(0);
    assert_eq!(project.entries().len(), 2, "and back again");
    assert!(!project.entries()[0].expanded);
}

#[test]
fn a_folder_that_is_not_a_repository_has_nothing_to_say_about_git() {
    let scratch = Scratch::new("nogit");
    scratch.file("notes.md", "hello");
    let project = Project::open(scratch.0.clone());
    assert!(project.entries().iter().all(|e| e.status.is_none()));
}

#[test]
fn a_new_file_in_a_repository_is_untracked_and_marks_its_folder() {
    let scratch = Scratch::new("git");
    if std::process::Command::new("git")
        .arg("-C")
        .arg(&scratch.0)
        .arg("init")
        .output()
        .is_err()
    {
        // No git on this machine: the feature degrades to no dots, and so
        // does the test.
        return;
    }
    scratch.file("chapters/one.md", "hello");

    let mut project = Project::open(scratch.0.clone());
    project.toggle(0);

    let folder = project
        .entries()
        .iter()
        .find(|e| e.name() == "chapters")
        .expect("the folder");
    let file = project
        .entries()
        .iter()
        .find(|e| e.name() == "one.md")
        .expect("the file");

    assert_eq!(file.status, Some(Status::Untracked));
    assert!(
        folder.status.is_some(),
        "a change anywhere marks every folder above it, so a collapsed tree still says where to look"
    );
}

#[test]
fn each_status_has_a_marker_and_clean_has_none() {
    assert!(!Status::Modified.marker().is_empty());
    assert!(!Status::Staged.marker().is_empty());
    assert!(!Status::Untracked.marker().is_empty());
    assert!(Status::Clean.marker().is_empty());
}

// -- searching the folder --------------------------------------------------

/// A folder with a few documents to find things in.
///
/// Named per test: the tests run at once, and `Scratch` is a fixed path.
fn searchable(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    scratch.file("one.md", "# Title\nthe needle is here\nand nothing else\n");
    scratch.file("two.md", "no match at all\n");
    scratch.file("three.txt", "needle again\nand a second needle\n");
    scratch.file("picture.png", "needle but not a document");
    scratch.file(".git/config", "needle in a skipped folder");
    scratch
}

fn query(text: &str, matching: Matching) -> Query {
    Query::new(text, matching).expect("a valid query")
}

#[test]
fn a_search_finds_every_match_with_its_line() {
    let scratch = searchable("search-lines");
    let hits = project::search(&scratch.0, &query("needle", Matching::Loose));
    assert_eq!(hits.len(), 3, "one in one.md, two in three.txt");
    let first = hits.iter().find(|h| h.path.ends_with("one.md")).unwrap();
    assert_eq!(first.line, 2);
    assert_eq!(first.text, "the needle is here");
}

#[test]
fn matches_are_numbered_within_their_own_file() {
    let scratch = searchable("search-ordinals");
    let hits = project::search(&scratch.0, &query("needle", Matching::Loose));
    let three: Vec<usize> = hits
        .iter()
        .filter(|h| h.path.ends_with("three.txt"))
        .map(|h| h.ordinal)
        .collect();
    assert_eq!(three, vec![0, 1], "each file counts from zero again");
}

#[test]
fn a_search_only_reads_documents() {
    let scratch = searchable("search-documents");
    let hits = project::search(&scratch.0, &query("needle", Matching::Loose));
    assert!(
        hits.iter().all(|h| !h.path.ends_with("picture.png")),
        "a png is not a document"
    );
}

#[test]
fn a_search_stays_out_of_dot_folders() {
    let scratch = searchable("search-hidden");
    let hits = project::search(&scratch.0, &query("needle", Matching::Loose));
    assert!(
        hits.iter()
            .all(|h| !h.path.to_string_lossy().contains(".git")),
        "nothing under .git"
    );
}

#[test]
fn a_search_matches_the_way_the_find_bar_does() {
    let scratch = searchable("search-matching");
    assert_eq!(
        project::search(&scratch.0, &query("Needle", Matching::Loose)).len(),
        3,
        "loose ignores case"
    );
    assert_eq!(
        project::search(&scratch.0, &query("Needle", Matching::CaseSensitive)).len(),
        0
    );
}

#[test]
fn results_come_back_in_path_then_line_order() {
    let scratch = searchable("search-order");
    let hits = project::search(&scratch.0, &query("needle", Matching::Loose));
    let ordered: Vec<_> = hits.iter().map(|h| (h.path.clone(), h.line)).collect();
    let mut sorted = ordered.clone();
    sorted.sort();
    assert_eq!(ordered, sorted);
}
