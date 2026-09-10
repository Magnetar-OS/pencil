// SPDX-License-Identifier: GPL-3.0-only

//! Persisted settings, through `cosmic-config` so they live alongside every
//! other COSMIC application's and are picked up live when changed.
//!
//! Deliberately small. The document is on disk; this is only what the window
//! should look like when it opens, and the handful of preferences that have an
//! argument behind them rather than a taste.

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

/// The smallest text size that is still text rather than a texture.
pub const MINIMUM_TEXT_SIZE: u16 = 9;

/// Above this a document stops being a document and becomes a slide.
pub const MAXIMUM_TEXT_SIZE: u16 = 32;

/// How the caret is drawn, as a setting rather than a type, so the value
/// persists across a version that adds a shape.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CaretShape {
    #[default]
    Line,
    Bar,
    Block,
    Underline,
}

impl CaretShape {
    #[must_use]
    pub fn all() -> [Self; 4] {
        [Self::Line, Self::Bar, Self::Block, Self::Underline]
    }

    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Line => crate::fl!("caret-line"),
            Self::Bar => crate::fl!("caret-bar"),
            Self::Block => crate::fl!("caret-block"),
            Self::Underline => crate::fl!("caret-underline"),
        }
    }
}

impl From<CaretShape> for nib::Caret {
    fn from(shape: CaretShape) -> Self {
        match shape {
            CaretShape::Line => Self::Line,
            CaretShape::Bar => Self::Bar,
            CaretShape::Block => Self::Block,
            CaretShape::Underline => Self::Underline,
        }
    }
}

// `cosmic-config` stores what it can serialise; an index is what a shape is on
// disk, and the conversion is here rather than at every call site.
impl From<u16> for CaretShape {
    fn from(index: u16) -> Self {
        Self::all()
            .get(index as usize)
            .copied()
            .unwrap_or_default()
    }
}

impl From<CaretShape> for u16 {
    fn from(shape: CaretShape) -> Self {
        Self::try_from(
            CaretShape::all()
                .iter()
                .position(|s| *s == shape)
                .unwrap_or(0),
        )
        .unwrap_or(0)
    }
}

/// Which theme the application asks for.
///
/// A setting rather than always following the desktop, because a writer who
/// works in a dark room and a desktop that follows sunrise disagree, and the
/// document is what is being looked at for hours.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::System, Self::Light, Self::Dark]
    }

    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::System => crate::fl!("theme-system"),
            Self::Light => crate::fl!("theme-light"),
            Self::Dark => crate::fl!("theme-dark"),
        }
    }

    /// The COSMIC theme this asks for.
    pub fn theme(self) -> cosmic::Theme {
        match self {
            Self::System => cosmic::theme::system_preference(),
            Self::Light => cosmic::Theme::light(),
            Self::Dark => cosmic::Theme::dark(),
        }
    }
}

impl From<u16> for Appearance {
    fn from(index: u16) -> Self {
        Self::all().get(index as usize).copied().unwrap_or_default()
    }
}

impl From<Appearance> for u16 {
    fn from(appearance: Appearance) -> Self {
        Self::try_from(
            Appearance::all()
                .iter()
                .position(|a| *a == appearance)
                .unwrap_or(0),
        )
        .unwrap_or(0)
    }
}

/// How many recently opened files are remembered.
///
/// Ten: enough to cover a working week's documents, short enough that the list
/// is still something to read rather than search.
pub const RECENT_LIMIT: usize = 10;

/// A pile of booleans, and deliberately so: each is an independent preference
/// the user sets or does not, and folding them into enums would invent
/// relationships between them that do not exist.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    /// The body text size, in points.
    pub text_size: u16,
    /// Which caret shape, by its index in [`CaretShape::all`].
    pub caret: u16,
    /// Whether the caret blinks. Off for anyone who finds it distracting, and
    /// for anyone recording their screen.
    pub caret_blinks: bool,
    /// Whether the caret glides between positions rather than jumping.
    pub caret_glides: bool,
    /// Whether the outline sidebar is showing.
    pub outline: bool,
    /// The width the text column is held to, in characters. Zero is the full
    /// width of the window.
    ///
    /// A measure rather than a window width: a line of prose past about
    /// eighty characters is one the eye loses its place returning from, and a
    /// maximised window on a wide screen is exactly how that happens.
    pub measure: u16,
    /// Which theme, by its index in [`Appearance::all`].
    pub appearance: u16,
    /// Recently opened files, most recent first.
    pub recent: Vec<String>,
    /// Whether code blocks get a gutter of line numbers.
    pub line_numbers: bool,
    /// Whether code blocks wrap. Off is what a programmer wants and on is what
    /// a reader wants, so it is a setting rather than a decision.
    pub wrap_code: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            text_size: 15,
            caret: 0,
            caret_blinks: true,
            caret_glides: true,
            outline: true,
            measure: 78,
            appearance: 0,
            recent: Vec::new(),
            line_numbers: false,
            wrap_code: true,
        }
    }
}

impl Config {
    /// The configuration handler for this application.
    ///
    /// # Errors
    ///
    /// [`cosmic_config::Error`] when the configuration directory cannot be
    /// opened.
    pub fn handler() -> Result<cosmic_config::Config, cosmic_config::Error> {
        cosmic_config::Config::new(crate::APP_ID, Self::VERSION)
    }

    #[must_use]
    pub fn caret_shape(&self) -> CaretShape {
        CaretShape::from(self.caret)
    }

    #[must_use]
    pub fn appearance(&self) -> Appearance {
        Appearance::from(self.appearance)
    }

    /// Puts a path at the head of the recent list, without duplicating it.
    pub fn remember(&mut self, path: &std::path::Path) {
        let path = path.display().to_string();
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(RECENT_LIMIT);
    }

    /// The remembered files that are still there.
    ///
    /// Checked on read rather than pruned on write: a file on a drive that is
    /// not mounted today should come back when it is, not be forgotten.
    #[must_use]
    pub fn recent_files(&self) -> Vec<std::path::PathBuf> {
        self.recent
            .iter()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
            .collect()
    }

    /// The engine's style for these settings, over a theme.
    #[must_use]
    pub fn style(&self, theme: &cosmic::Theme) -> nib::Style {
        let mut style = nib::Style::from_theme(theme);
        style.text_size = f32::from(
            self.text_size
                .clamp(MINIMUM_TEXT_SIZE, MAXIMUM_TEXT_SIZE),
        );
        style.caret = self.caret_shape().into();
        if !self.caret_blinks {
            style.blink_period = std::time::Duration::ZERO;
        }
        if !self.caret_glides {
            style.caret_glide = std::time::Duration::ZERO;
        }
        style.line_numbers = self.line_numbers;
        style.wrap_code = self.wrap_code;
        style
    }

    /// The width the text column is held to, in pixels, or `None` for the
    /// window's own width.
    #[must_use]
    pub fn column_width(&self) -> Option<f32> {
        (self.measure > 0).then(|| {
            // A rough advance per character: the measure is a reading comfort
            // setting, not a typographic one, and to the character is close
            // enough for it.
            f32::from(self.measure) * f32::from(self.text_size) * 0.55
        })
    }
}
