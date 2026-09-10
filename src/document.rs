// SPDX-License-Identifier: GPL-3.0-only

//! Reading and writing files.
//!
//! # One document, several spellings
//!
//! Pencil edits a Nib document. Markdown, HTML and plain text are ways of
//! writing one down, not different kinds of document — so opening a `.md` and
//! saving it as `.html` is not a conversion, it is the same document written
//! differently. That is the whole reason the engine has a model at the centre
//! rather than a text buffer.
//!
//! # What is lost, and where
//!
//! Every format loses something the others keep, and the honest place to say
//! so is here:
//!
//! - **Markdown** has no underline. A document with underlined text saved as
//!   Markdown comes back without it.
//! - **Plain text** loses every mark and every structure but the shape of the
//!   blocks.
//! - **HTML** loses nothing the schema holds, and is the format to save in
//!   when the document matters more than the file being readable.
//!
//! Pencil does not warn about this on every save — a dialog nobody reads is
//! worse than a footnote — but the status bar names the format, and Save As
//! offers HTML first for a document that has something to lose.

use std::fmt;
use std::path::{Path, PathBuf};

use nib_html::Html;
use nib_markdown::{Dialect, Markdown};
use nib_model::node::Node;
use nib_model::schema::Schema;
use nib_text::Text;

/// How a document is written down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Markdown,
    /// Markdown with Nuxt's component syntax.
    Mdc,
    /// Markdown with JSX-shaped components.
    Mdx,
    Html,
    Text,
}

impl Format {
    /// The format a path's extension names, defaulting to Markdown.
    #[must_use]
    pub fn of(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("html" | "htm" | "xhtml") => Self::Html,
            Some("txt" | "text" | "log") => Self::Text,
            Some("mdc") => Self::Mdc,
            Some("mdx") => Self::Mdx,
            _ => Self::Markdown,
        }
    }

    /// The extension a new file of this format gets.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Mdc => "mdc",
            Self::Mdx => "mdx",
            Self::Html => "html",
            Self::Text => "txt",
        }
    }

    /// Whether saving in this format keeps everything the schema holds.
    #[must_use]
    pub fn lossless(self) -> bool {
        matches!(self, Self::Html)
    }

    /// Every format, for a Save As filter list.
    #[must_use]
    pub fn all() -> [Self; 5] {
        [Self::Html, Self::Markdown, Self::Mdc, Self::Mdx, Self::Text]
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Markdown => "Markdown",
            Self::Mdc => "MDC",
            Self::Mdx => "MDX",
            Self::Html => "HTML",
            Self::Text => "Plain text",
        })
    }
}

/// Why a document could not be read or written.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("{path}: {message}")]
    Io { path: String, message: String },
    #[error("{0} is not valid UTF-8")]
    Encoding(String),
}

impl Error {
    fn io(path: &Path, error: &std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }
}

/// The converters for one schema, built once.
#[derive(Debug, Clone)]
pub struct Converters {
    schema: Schema,
    html: Html,
    text: Text,
}

impl Converters {
    #[must_use]
    pub fn new(schema: &Schema) -> Self {
        Self {
            schema: schema.clone(),
            html: Html::new(schema),
            text: Text::new(schema),
        }
    }

    fn markdown(&self, format: Format) -> Markdown {
        let dialect = match format {
            Format::Mdc => Dialect::Mdc,
            Format::Mdx => Dialect::Mdx,
            _ => Dialect::Gfm,
        };
        Markdown::new(&self.schema, dialect)
    }

    /// Reads a string in a format into a document.
    #[must_use]
    pub fn parse(&self, format: Format, source: &str) -> Node {
        match format {
            Format::Html => self.html.parse(source),
            Format::Text => self.text.parse(source),
            other => self.markdown(other).parse(source),
        }
    }

    /// Writes a document in a format.
    #[must_use]
    pub fn write(&self, format: Format, doc: &Node) -> String {
        match format {
            Format::Html => self.html.to_html(doc),
            Format::Text => self.text.to_text(doc),
            other => self.markdown(other).to_markdown(doc),
        }
    }
}

/// Reads a file.
///
/// # Errors
///
/// [`Error`] when the file cannot be read or is not UTF-8.
pub async fn read(path: PathBuf) -> Result<(PathBuf, Format, String), Error> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| Error::io(&path, &e))?;
    let source =
        String::from_utf8(bytes).map_err(|_| Error::Encoding(path.display().to_string()))?;
    let format = Format::of(&path);
    Ok((path, format, source))
}

/// Writes a file.
///
/// Writes to a temporary file beside the target and renames over it, so an
/// interrupted save leaves the previous version rather than half of the new
/// one. A crash mid-write is the one time a text editor can lose a day's work,
/// and a rename is atomic on every filesystem this will meet.
///
/// # Errors
///
/// [`Error`] when the file cannot be written.
pub async fn write(path: PathBuf, contents: String) -> Result<PathBuf, Error> {
    let temporary = path.with_extension(format!(
        "{}.pencil-tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    tokio::fs::write(&temporary, contents.as_bytes())
        .await
        .map_err(|e| Error::io(&temporary, &e))?;
    tokio::fs::rename(&temporary, &path)
        .await
        .map_err(|e| Error::io(&path, &e))?;
    Ok(path)
}

/// The name shown in the title bar.
#[must_use]
pub fn title(path: Option<&Path>) -> String {
    path.and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map_or_else(|| crate::fl!("untitled"), str::to_owned)
}
