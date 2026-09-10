// SPDX-License-Identifier: GPL-3.0-only

//! Printing: what comes out is a PDF, and it has the document in it.

use nib_model::basic;
use pencil::document::{Converters, Format};
use pencil::print::{Sheet, to_pdf};

const SAMPLE: &str = "\
# A Heading

A paragraph with **bold** and *italic* and `code` in it.

- first bullet
- second bullet
  - nested

> a quotation

```rust
fn main() {}
```

| one | two |
| --- | --- |
| a   | b   |

---

The end.
";

fn pdf(source: &str) -> Vec<u8> {
    let schema = basic::schema();
    let converters = Converters::new(&schema);
    let doc = converters.parse(Format::Markdown, source);
    to_pdf(&doc, "test", Sheet::default())
}

#[test]
fn it_produces_a_pdf() {
    let bytes = pdf(SAMPLE);
    assert!(bytes.starts_with(b"%PDF-"), "a PDF header");
    assert!(bytes.ends_with(b"%%EOF\n") || bytes.ends_with(b"%%EOF"));
    assert!(bytes.len() > 5000, "a font was embedded, not just a stub");
}

#[test]
fn every_page_is_a4() {
    let bytes = pdf(SAMPLE);
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("/MediaBox [0 0 595.28 841.89]"),
        "A4, in points"
    );
}

#[test]
fn the_fonts_are_embedded_not_just_named() {
    let bytes = pdf(SAMPLE);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/FontFile2"), "the font file itself is in it");
    assert!(text.contains("/Identity-H"), "two bytes per glyph");
    assert!(text.contains("/ToUnicode"), "so the text can be copied out");
}

#[test]
fn a_long_document_runs_onto_more_pages() {
    let long = "A paragraph.\n\n".repeat(200);
    let bytes = pdf(&long);
    let text = String::from_utf8_lossy(&bytes);
    let count: usize = text
        .split("/Count ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("a page count");
    assert!(count > 1, "200 paragraphs do not fit on one page, got {count}");
}

#[test]
fn an_empty_document_still_makes_a_page() {
    let bytes = pdf("");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/Count 1"), "one page, blank");
}

#[test]
fn the_title_reaches_the_file() {
    let schema = basic::schema();
    let converters = Converters::new(&schema);
    let doc = converters.parse(Format::Markdown, "hello");
    let bytes = to_pdf(&doc, "notes.md", Sheet::default());
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("notes.md"), "the print dialog's job name");
}
