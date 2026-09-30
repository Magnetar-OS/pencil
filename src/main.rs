// SPDX-License-Identifier: GPL-3.0-only

//! Pencil — a rich text editor for the COSMIC desktop.

use pencil::launch::{Arguments, Flags};
use pencil::{app, i18n, recovery};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The desktop's languages, not the process's locale: a COSMIC session sets
    // a list, and the first one this application has strings for is the one to
    // use.
    i18n::init(&i18n_embed::DesktopLanguageRequester::requested_languages());

    // The files and folders named on the command line; the desktop entry
    // passes local paths (`%F`). `Flags` makes them absolute, so the
    // recent-files list and the window they are handed to name the same files
    // whatever directory each runs in. A path that does not exist yet is a new
    // document to be saved there.
    let arguments = Arguments::parse(std::env::args_os().skip(1));
    let flags = Flags::new(arguments.paths, recovery::default_dir());

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
    if arguments.new_window {
        // A window of its own, asked for by name: it does not look for one
        // that is already running, and does not take the bus name.
        cosmic::app::run::<app::App>(settings, flags)?;
    } else {
        // `run_single_instance` rather than `run`: when a window is already
        // up it is handed the flags over the session bus and this process
        // exits, so a file opened from the file manager becomes a tab there
        // instead of a second window.
        cosmic::app::run_single_instance::<app::App>(settings, flags)?;
    }
    Ok(())
}
