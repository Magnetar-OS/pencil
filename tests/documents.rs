// SPDX-License-Identifier: GPL-3.0-only

//! What survives a save and a reopen.
//!
//! The claim the application makes in its own about pane is that the document
//! is a document rather than a text buffer, and that saving as HTML and
//! reopening gives the same thing back. These are the tests that hold it to
//! that, and that say plainly what each of the other formats loses.

use std::path::Path;

use nib_model::basic;
use pencil::document::{Converters, Format};

fn converters() -> Converters {
    Converters::new(&basic::schema())
}

/// Writes a document in a format and reads it back.
fn round(format: Format, source: &str) -> String {
    let c = converters();
    let doc = c.parse(Format::Markdown, source);
    let written = c.write(format, &doc);
    let back = c.parse(format, &written);
    back.to_string()
}

fn shape(source: &str) -> String {
    converters().parse(Format::Markdown, source).to_string()
}

#[test]
fn html_gives_the_same_document_back() {
    for source in [
        "# Title\n\nA paragraph with **bold** and *emphasis*.\n",
        "- one\n- two\n",
        "1. first\n2. second\n",
        "> quoted\n",
        "```rust\nfn main() {}\n```\n",
        "| a | b |\n| --- | --- |\n| 1 | 2 |\n",
        "A [link](http://example.test) and `code`.\n",
        "---\n",
    ] {
        assert_eq!(
            round(Format::Html, source),
            shape(source),
            "HTML lost something from {source:?}"
        );
    }
}

#[test]
fn markdown_gives_the_same_document_back_for_what_it_can_hold() {
    for source in [
        "# Title\n",
        "A paragraph with **bold** and *emphasis*.\n",
        "- one\n- two\n",
        "> quoted\n",
        "```rust\nfn main() {}\n```\n",
    ] {
        assert_eq!(
            round(Format::Markdown, source),
            shape(source),
            "Markdown lost something from {source:?}"
        );
    }
}

#[test]
fn markdown_has_no_underline_and_says_so_by_losing_it() {
    let c = converters();
    let doc = c.parse(Format::Html, "<p><u>under</u></p>");
    assert!(doc.to_string().contains("underline"));

    let back = c.parse(Format::Markdown, &c.write(Format::Markdown, &doc));
    assert!(
        !back.to_string().contains("underline"),
        "this is the loss the format's documentation promises"
    );
    assert_eq!(back.text_content(), "under", "but the words survive");
}

#[test]
fn plain_text_keeps_the_words_and_the_blocks() {
    let c = converters();
    let doc = c.parse(
        Format::Markdown,
        "# Title\n\nA paragraph.\n\n- one\n- two\n",
    );
    let text = c.write(Format::Text, &doc);
    for word in ["Title", "A paragraph.", "one", "two"] {
        assert!(text.contains(word), "{word:?} is missing from {text:?}");
    }
    let back = c.parse(Format::Text, &text);
    assert_eq!(back.check(), Ok(()));
}

#[test]
fn a_format_is_chosen_by_extension() {
    assert_eq!(Format::of(Path::new("a.md")), Format::Markdown);
    assert_eq!(Format::of(Path::new("a.markdown")), Format::Markdown);
    assert_eq!(Format::of(Path::new("a.html")), Format::Html);
    assert_eq!(Format::of(Path::new("a.HTM")), Format::Html);
    assert_eq!(Format::of(Path::new("a.txt")), Format::Text);
    assert_eq!(Format::of(Path::new("a.mdc")), Format::Mdc);
    assert_eq!(Format::of(Path::new("a.mdx")), Format::Mdx);
    // Anything unrecognised is prose, which is the safe guess for an editor.
    assert_eq!(Format::of(Path::new("notes")), Format::Markdown);
}

#[test]
fn only_html_claims_to_be_lossless() {
    assert!(Format::Html.lossless());
    for format in [Format::Markdown, Format::Mdc, Format::Mdx, Format::Text] {
        assert!(!format.lossless(), "{format} should not claim to be lossless");
    }
}

#[test]
fn changing_the_format_changes_the_file_but_not_the_document() {
    // Opening a `.md` and saving as `.html` is not a conversion; it is the
    // same document written differently.
    let c = converters();
    let source = "# Title\n\nA paragraph with **bold**.\n";
    let doc = c.parse(Format::Markdown, source);
    let as_html = c.write(Format::Html, &doc);
    let as_markdown = c.write(Format::Markdown, &doc);

    assert_ne!(as_html, as_markdown, "the files differ");
    assert_eq!(
        c.parse(Format::Html, &as_html).to_string(),
        c.parse(Format::Markdown, &as_markdown).to_string(),
        "the documents do not"
    );
}

#[tokio::test]
async fn a_save_is_atomic_and_leaves_no_temporary_behind() {
    let dir = std::env::temp_dir().join("pencil-test-save");
    let _ = tokio::fs::create_dir_all(&dir).await;
    let path = dir.join("document.md");

    pencil::document::write(path.clone(), "# One\n".to_owned())
        .await
        .expect("the first write");
    pencil::document::write(path.clone(), "# Two\n".to_owned())
        .await
        .expect("the second write");

    let contents = tokio::fs::read_to_string(&path).await.expect("read back");
    assert_eq!(contents, "# Two\n");

    let mut entries = tokio::fs::read_dir(&dir).await.expect("list");
    let mut names = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    assert_eq!(names, ["document.md"], "the temporary file was renamed away");
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn reading_a_file_that_is_not_there_says_which() {
    let error = pencil::document::read("/nonesuch/document.md".into())
        .await
        .expect_err("there is no such file");
    assert!(
        error.to_string().contains("/nonesuch/document.md"),
        "the message should name the file: {error}"
    );
}
