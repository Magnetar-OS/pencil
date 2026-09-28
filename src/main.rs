// SPDX-License-Identifier: GPL-3.0-only

//! Pencil — a rich text editor for the COSMIC desktop.

use std::path::PathBuf;

use pencil::{app, i18n};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The desktop's languages, not the process's locale: a COSMIC session sets
    // a list, and the first one this application has strings for is the one to
    // use.
    i18n::init(&i18n_embed::DesktopLanguageRequester::requested_languages());

    // A file or folder named on the command line, if there is one. The
    // desktop entry passes a local path (`%f`). Made absolute here, so the
    // recent-files list and a second window started from this one name the
    // same file whatever directory they run in. A path that does not exist
    // yet is a new document to be saved there.
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .map(|path| std::path::absolute(&path).unwrap_or(path));

    // Closing the window is a request the application answers, not a close:
    // unsaved tabs are asked about first (`App::quit`).
    let settings = cosmic::app::Settings::default()
        .exit_on_close(false)
        .size(cosmic::iced::Size::new(1000.0, 760.0))
        .size_limits(
            cosmic::iced::Limits::NONE
                .min_width(480.0)
                .min_height(360.0),
        );
    cosmic::app::run::<app::App>(settings, path)?;
    Ok(())
}
