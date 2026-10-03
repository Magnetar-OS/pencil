// SPDX-License-Identifier: GPL-3.0-only

//! What a launch asks for, and how a second launch hands it to the first.
//!
//! Pencil is one window per process, and the first process owns the
//! application's name on the session bus. `magnetar-pencil notes.md` run
//! while that window is up does not start a second one: libcosmic's
//! `run_single_instance` sends the running window this launch's [`Flags`] and
//! exits, and the file opens there as a tab. **New window** is the one launch
//! that means a second window, and says so with [`NEW_WINDOW`].
//!
//! What crosses the bus is a list of strings, so the paths travel as absolute
//! `file://` URLs: the second process's working directory is not the first's,
//! and a file name is bytes, not necessarily UTF-8.

use std::ffi::OsString;
use std::path::PathBuf;

/// The argument that asks for a window of its own.
pub const NEW_WINDOW: &str = "--new-window";

/// The action a second launch sends when it has something to open. A launch
/// with nothing to open sends none, and the running window is only raised.
pub const OPEN: &str = "open";

/// The command line, read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arguments {
    /// A window of its own, whether or not one is already running.
    pub new_window: bool,
    /// The files and folders named, as given.
    pub paths: Vec<PathBuf>,
}

impl Arguments {
    /// Reads the arguments after the program's name.
    ///
    /// Everything but [`NEW_WINDOW`] is a path: a name that does not exist
    /// yet is a document to be started there.
    #[must_use]
    pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Self {
        let mut parsed = Self::default();
        for argument in arguments {
            if argument == NEW_WINDOW {
                parsed.new_window = true;
            } else {
                parsed.paths.push(PathBuf::from(argument));
            }
        }
        parsed
    }
}

/// What the application starts with.
#[derive(Clone, Debug, Default)]
pub struct Flags {
    /// What to open, as absolute `file://` URLs.
    open: Vec<String>,
    /// [`OPEN`] when there is something to open. Decided here rather than by
    /// the running window because libcosmic picks the bus method from it.
    action: Option<String>,
    /// Where unsaved documents are kept between launches, or `None` to keep
    /// none. See [`crate::recovery`].
    pub recovery: Option<PathBuf>,
}

impl Flags {
    /// Flags for opening `paths`, resolved against this process's working
    /// directory.
    #[must_use]
    pub fn new(paths: impl IntoIterator<Item = PathBuf>, recovery: Option<PathBuf>) -> Self {
        let open: Vec<String> = paths
            .into_iter()
            .filter_map(|path| {
                // Absolute, not canonical: a symlink stays the name the user
                // gave, which is the name the document is saved under.
                let path = std::path::absolute(&path).unwrap_or(path);
                // Only a path that is not absolute has no URL, and the only
                // one `absolute` leaves that way is the empty argument, which
                // names nothing.
                url::Url::from_file_path(path).ok().map(String::from)
            })
            .collect();
        Self {
            action: (!open.is_empty()).then(|| OPEN.to_owned()),
            open,
            recovery,
        }
    }

    /// The files and folders to open.
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        paths(&self.open)
    }
}

/// The paths a list of `file://` URLs names. Anything else in the list is not
/// a file on this computer and is left out.
pub fn paths<S: AsRef<str>>(urls: &[S]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|url| url::Url::parse(url.as_ref()).ok())
        .filter_map(|url| url.to_file_path().ok())
        .collect()
}

impl cosmic::app::CosmicFlags for Flags {
    type SubCommand = String;
    type Args = Vec<String>;

    fn action(&self) -> Option<&Self::SubCommand> {
        self.action.as_ref()
    }

    fn args(&self) -> Vec<&str> {
        self.open.iter().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt as _;

    use cosmic::app::CosmicFlags as _;

    use super::*;

    fn arguments(given: &[&str]) -> Arguments {
        Arguments::parse(given.iter().map(OsString::from))
    }

    #[test]
    fn every_argument_is_a_path_but_the_one_that_asks_for_a_window() {
        let parsed = arguments(&["notes.md", NEW_WINDOW, "/srv/docs"]);
        assert!(parsed.new_window);
        assert_eq!(
            parsed.paths,
            [PathBuf::from("notes.md"), PathBuf::from("/srv/docs")]
        );
        assert!(!arguments(&["notes.md"]).new_window);
    }

    /// Nothing to open: the second launch sends a plain activation, which
    /// raises the running window and nothing more.
    #[test]
    fn a_bare_launch_asks_the_running_window_for_nothing() {
        let flags = Flags::new(Vec::new(), None);
        assert!(flags.action().is_none());
        assert_eq!(flags.args(), Vec::<&str>::new());
    }

    /// The paths arrive in the running window as the files they named here,
    /// whatever directory that window was started in.
    #[test]
    fn paths_are_handed_over_absolute() {
        let flags = Flags::new(
            [PathBuf::from("notes.md"), PathBuf::from("/srv/docs/a b.md")],
            None,
        );
        assert_eq!(flags.action().map(String::as_str), Some(OPEN));

        let here = std::env::current_dir().unwrap();
        // What the running window does with what it is sent.
        let received: Vec<String> = flags.args().into_iter().map(str::to_owned).collect();
        assert_eq!(
            paths(&received),
            [here.join("notes.md"), PathBuf::from("/srv/docs/a b.md")]
        );
        assert_eq!(flags.paths(), paths(&received));
    }

    /// A file name is bytes. One that is not UTF-8, or that holds a newline,
    /// still crosses the bus — which carries only strings — and comes out the
    /// same name.
    #[test]
    fn a_name_that_is_not_text_survives_the_handover() {
        let odd = PathBuf::from(OsString::from_vec(b"/tmp/caf\xe9\n100%.md".to_vec()));
        let flags = Flags::new([odd.clone()], None);
        assert_eq!(flags.args().len(), 1);
        assert_eq!(flags.paths(), [odd]);
    }

    /// What is not a local file is left out rather than guessed at.
    #[test]
    fn what_is_not_a_local_file_is_not_opened() {
        assert_eq!(
            paths(&["https://example.test/a.md", "notes.md", ""]),
            Vec::<PathBuf>::new()
        );
        assert!(Flags::new([PathBuf::new()], None).action().is_none());
    }
}
