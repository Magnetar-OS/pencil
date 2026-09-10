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
