// SPDX-License-Identifier: GPL-3.0-only

//! Pencil — a rich text editor for the COSMIC desktop.

use std::path::PathBuf;

use pencil::{app, i18n};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The desktop's languages, not the process's locale: a COSMIC session sets
    // a list, and the first one this application has strings for is the one to
    // use.
    i18n::init(&i18n_embed::DesktopLanguageRequester::requested_languages());

    // A file named on the command line, if there is one. Everything else is
    // ignored rather than refused: a desktop launcher passes a URL, and a
    // shell passes whatever it was given.
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.exists());

    let settings = cosmic::app::Settings::default()
        .size(cosmic::iced::Size::new(1000.0, 760.0))
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(480.0)
                .min_height(360.0),
        );
    cosmic::app::run::<app::App>(settings, path)?;
    Ok(())
}
