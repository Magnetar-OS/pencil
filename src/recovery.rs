// SPDX-License-Identifier: GPL-3.0-only

//! Unsaved documents, kept where a crash cannot reach them.
//!
//! # What is kept, and for how long
//!
//! A tab with unsaved changes has one *snapshot*: the document as it stood a
//! few seconds ago, in a file of its own. The snapshot is removed the moment
//! it stops being the only copy — the tab is saved, or closed, or its changes
//! are knowingly discarded. So a snapshot that is still there when Pencil
//! starts is, by construction, work somebody did not get to save.
//!
//! # Layout
//!
//! ```text
//! $XDG_STATE_HOME/pencil/recovery/     0700
//!   lock                               serialises starting up
//!   <session>/                         0700, one per running window
//!     lock                             held for as long as the window lives
//!     <n>.snapshot                     0600, one per unsaved tab
//! ```
//!
//! # Whose snapshots are whose
//!
//! A Pencil window is a process, and several run at once. Each takes a
//! directory and holds a lock on the file in it until it exits. A directory
//! whose lock can be *taken* therefore belongs to a window that went away
//! without cleaning up — it crashed, was killed, or lost power — and what it
//! holds is what gets offered back. Nothing else tells a dead window's
//! snapshots from those of the window next to this one, which must be left
//! alone. The window that finds them keeps the lock, so a second window
//! starting a moment later does not offer the same documents again.
//!
//! # What a snapshot holds
//!
//! A few header lines, a blank line, and the document as HTML — the one format
//! that loses nothing the schema holds (see [`crate::document`]), whatever
//! format the document itself is saved in. Header and document are one file,
//! written whole: there is no state in which one is on disk without the other.
//!
//! # What a write guarantees
//!
//! A snapshot is written to a temporary file beside it, flushed to the disk,
//! and renamed into place, so a crash during the write — the very thing this
//! exists for — leaves the previous snapshot rather than half of the new one.
//! Files are created `0600` and directories `0700`: an unsaved document is at
//! least as private as the saved one, and its owner never chose its mode.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::document::Format;

/// The first line of every snapshot. The number is the layout's version.
const MAGIC: &str = "pencil-recovery 1";

/// What a snapshot's file name ends in.
const SNAPSHOT: &str = "snapshot";

/// The lock file in the recovery directory and in each session's.
const LOCK: &str = "lock";

/// Why a snapshot could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("not a recovery file: {0}")]
    Malformed(&'static str),
}

/// One unsaved document, as read back from its snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The snapshot's own file.
    pub file: PathBuf,
    /// The file the document was opened from or last saved to, if it had one.
    pub path: Option<PathBuf>,
    /// The format the document is saved in — not the snapshot's, which is
    /// always HTML.
    pub format: Format,
    /// When the snapshot was taken.
    pub taken: SystemTime,
    /// The document, as HTML.
    pub body: String,
}

impl Snapshot {
    /// Whether the document's file was written after this snapshot was taken:
    /// saved from another window, edited elsewhere, or saved a moment before
    /// the crash. Restoring would bring back text older than the file's, so
    /// the offer says so.
    #[must_use]
    pub fn changed_since(&self) -> bool {
        self.path
            .as_deref()
            .and_then(|path| fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok())
            .is_some_and(|modified| modified > self.taken)
    }
}

/// The text of a snapshot: the header, then the document as HTML.
#[must_use]
pub fn render(path: Option<&Path>, format: Format, taken: SystemTime, html: &str) -> String {
    let nanos = taken
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut text = format!("{MAGIC}\nformat={}\ntaken={nanos}\n", format.extension());
    // A `file://` URL rather than the path: it is one line whatever the name
    // holds, a newline or bytes that are not UTF-8 included.
    if let Some(url) = path.and_then(|path| url::Url::from_file_path(path).ok()) {
        text.push_str("path=");
        text.push_str(url.as_str());
        text.push('\n');
    }
    text.push('\n');
    text.push_str(html);
    text
}

/// Reads a snapshot back.
///
/// # Errors
///
/// [`Error`] when the file cannot be read or is not a snapshot.
pub fn read(file: &Path) -> Result<Snapshot, Error> {
    let bytes = fs::read(file)?;
    let text = String::from_utf8(bytes).map_err(|_| Error::Malformed("it is not UTF-8"))?;
    let (header, body) = text
        .split_once("\n\n")
        .ok_or(Error::Malformed("it has no document"))?;
    let mut lines = header.lines();
    if lines.next() != Some(MAGIC) {
        return Err(Error::Malformed("it does not start as one"));
    }

    let mut path = None;
    let mut format = None;
    let mut taken = None;
    for line in lines {
        match line.split_once('=') {
            Some(("path", value)) => {
                path = Some(
                    url::Url::parse(value)
                        .ok()
                        .and_then(|url| url.to_file_path().ok())
                        .ok_or(Error::Malformed("its path is not a file"))?,
                );
            }
            Some(("format", value)) => {
                format = Format::all()
                    .into_iter()
                    .find(|format| format.extension() == value);
            }
            Some(("taken", value)) => {
                taken = value
                    .parse::<u64>()
                    .ok()
                    .map(|nanos| UNIX_EPOCH + Duration::from_nanos(nanos));
            }
            // A key from a later version: not this version's to understand,
            // and not a reason to refuse somebody's unsaved document.
            _ => {}
        }
    }

    Ok(Snapshot {
        file: file.to_path_buf(),
        path,
        format: format.ok_or(Error::Malformed("it names no format"))?,
        taken: taken.ok_or(Error::Malformed("it does not say when it was taken"))?,
        body: body.to_owned(),
    })
}

/// Writes a snapshot, whole or not at all.
///
/// # Errors
///
/// [`io::Error`] when the snapshot cannot be written. Whatever was at `file`
/// before is still there, untouched.
pub fn write(file: &Path, contents: &str) -> io::Result<()> {
    let temporary = temporary_for(file);
    // `create_new`, with the mode given up front: the file never exists with
    // any other permissions, and a name that is somehow taken is refused
    // rather than written through.
    let mut staged = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let landed = staged
        .write_all(contents.as_bytes())
        // To the disk, not the page cache: a snapshot that a power cut can
        // still lose is not one.
        .and_then(|()| staged.sync_all())
        .and_then(|()| fs::rename(&temporary, file));
    if let Err(error) = landed {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    // The rename is a change to the directory, with a journal entry of its
    // own; without this the name can be lost though the contents were kept.
    if let Some(parent) = file.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Removes a snapshot. One that is already gone is not an error.
///
/// # Errors
///
/// [`io::Error`] when the file is there and cannot be removed.
pub fn discard(file: &Path) -> io::Result<()> {
    match fs::remove_file(file) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Where a snapshot is staged before it is renamed into place.
///
/// Named after the process as well as the snapshot, so the name is this
/// write's alone and `create_new` means what it says.
fn temporary_for(file: &Path) -> PathBuf {
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    file.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

/// `$XDG_STATE_HOME/pencil/recovery`, or the same under `~/.local/state`.
///
/// `None` when neither is set, in which case nothing is kept: a recovery copy
/// in a directory chosen by guesswork is one nobody will find.
#[must_use]
pub fn default_dir() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))?;
    Some(state.join("pencil").join("recovery"))
}

/// What windows that are gone left behind.
#[derive(Debug, Default)]
pub struct Abandoned {
    /// The unsaved documents, oldest first.
    pub snapshots: Vec<Snapshot>,
    /// Files that are where a snapshot would be and will not read, with the
    /// reason. They are left where they are.
    pub unreadable: Vec<(PathBuf, String)>,
}

/// This window's place in the recovery directory.
#[derive(Debug)]
pub struct Session {
    dir: PathBuf,
    /// Held until the window exits; its release is how the next one knows
    /// this one is gone.
    _lock: File,
    next: u64,
    /// The directories of dead windows this one found, with their locks, so
    /// no other window offers the same documents.
    claimed: Vec<(PathBuf, File)>,
}

impl Session {
    /// Starts a session in `root`, and claims what dead ones left there.
    ///
    /// # Errors
    ///
    /// [`io::Error`] when the recovery directory cannot be created, locked or
    /// listed. Nothing is kept for this window then, and the caller says so.
    pub fn begin(root: &Path) -> io::Result<(Self, Abandoned)> {
        directory().recursive(true).create(root)?;

        // Starting up is the one moment two windows can mistake each other:
        // a directory that exists and is not locked *yet* looks exactly like
        // one whose window died. One window at a time through this function
        // closes that gap.
        let gate = File::create(root.join(LOCK))?;
        gate.lock()?;

        let mut abandoned = Abandoned::default();
        let mut claimed = Vec::new();
        for entry in fs::read_dir(root)? {
            let dir = entry?.path();
            if !dir.is_dir() {
                continue;
            }
            let lock = File::create(dir.join(LOCK))?;
            match lock.try_lock() {
                Ok(()) => {}
                // Somebody holds it: that window is alive, and its snapshots
                // are its own business.
                Err(fs::TryLockError::WouldBlock) => continue,
                Err(fs::TryLockError::Error(error)) => return Err(error),
            }
            collect(&dir, &mut abandoned)?;
            claimed.push((dir, lock));
        }
        abandoned.snapshots.sort_by_key(|snapshot| snapshot.taken);

        let dir = new_directory(root)?;
        let lock = File::create(dir.join(LOCK))?;
        lock.lock()?;

        let mut session = Self {
            dir,
            _lock: lock,
            next: 0,
            claimed,
        };
        session.sweep();
        Ok((session, abandoned))
    }

    /// A file for a tab's snapshot, used by no other.
    pub fn slot(&mut self) -> PathBuf {
        self.next += 1;
        self.dir.join(format!("{}.{SNAPSHOT}", self.next))
    }

    /// Takes over a dead window's snapshot as one of this window's own, for
    /// the tab it was restored into.
    ///
    /// # Errors
    ///
    /// [`io::Error`] when the file cannot be moved. It stays where it was.
    pub fn adopt(&mut self, file: &Path) -> io::Result<PathBuf> {
        let slot = self.slot();
        fs::rename(file, &slot)?;
        self.sweep();
        Ok(slot)
    }

    /// Lets go of the dead windows' directories that hold nothing any more.
    ///
    /// One that still holds a snapshot — undecided, or unreadable — is kept,
    /// locked, and found again by the next window to start after this one
    /// exits.
    pub fn sweep(&mut self) {
        self.claimed.retain(|(dir, _lock)| {
            let snapshots = fs::read_dir(dir).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == SNAPSHOT)
                })
            });
            // Best effort: a directory that will not go is found, empty, by
            // the next window, which tries again.
            snapshots || fs::remove_dir_all(dir).is_err()
        });
    }

    /// Ends the session: the window is closing, and every document in it has
    /// been saved or knowingly discarded.
    ///
    /// # Errors
    ///
    /// [`io::Error`] when the session's directory cannot be removed. What is
    /// left in it is then offered back on the next launch.
    pub fn end(self) -> io::Result<()> {
        fs::remove_dir_all(&self.dir)
    }
}

/// A directory only its owner can enter.
fn directory() -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
}

/// A directory in `root` that did not exist before.
fn new_directory(root: &Path) -> io::Result<PathBuf> {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut attempt = 0_u32;
    loop {
        let dir = root.join(format!("{}-{started}-{attempt}", std::process::id()));
        match directory().create(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => attempt += 1,
            Err(error) => return Err(error),
        }
    }
}

/// Reads what a dead window's directory holds.
///
/// A write that was interrupted left a temporary file beside the snapshot it
/// was replacing. It is half a document at best and is removed; the snapshot
/// itself is whole, because the rename never happened.
fn collect(dir: &Path, abandoned: &mut Abandoned) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let file = entry?.path();
        match file.extension().and_then(|extension| extension.to_str()) {
            Some(SNAPSHOT) => match read(&file) {
                Ok(snapshot) => abandoned.snapshots.push(snapshot),
                Err(error) => abandoned.unreadable.push((file, error.to_string())),
            },
            Some("tmp") => fs::remove_file(&file)?,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// A scratch recovery directory, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "pencil-test-recovery-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// A window that keeps a snapshot and then dies without ending its
    /// session: dropping the session releases its lock, which is all a crash
    /// does.
    fn crashed_with(root: &Path, contents: &str) -> PathBuf {
        let (mut session, _) = Session::begin(root).unwrap();
        let file = session.slot();
        write(&file, contents).unwrap();
        file
    }

    #[test]
    fn a_snapshot_comes_back_as_it_was_kept() {
        let scratch = Scratch::new("round-trip");
        // A name no header line could hold as it is.
        let path = PathBuf::from("/tmp/notes\nfor \u{3b1}.md");
        let taken = UNIX_EPOCH + Duration::new(1_790_000_000, 123_456_789);
        let body = "<h1>Title</h1>\n\n<p>Two blank lines above stay in the document.</p>";
        let file = crashed_with(
            &scratch.0,
            &render(Some(&path), Format::Markdown, taken, body),
        );

        let (_session, abandoned) = Session::begin(&scratch.0).unwrap();
        assert!(abandoned.unreadable.is_empty());
        assert_eq!(
            abandoned.snapshots,
            [Snapshot {
                file,
                path: Some(path),
                format: Format::Markdown,
                taken,
                body: body.to_owned(),
            }]
        );
    }

    #[test]
    fn an_untitled_document_comes_back_without_a_path() {
        let scratch = Scratch::new("untitled");
        crashed_with(
            &scratch.0,
            &render(None, Format::Html, SystemTime::now(), "<p>x</p>"),
        );
        let (_session, abandoned) = Session::begin(&scratch.0).unwrap();
        assert_eq!(abandoned.snapshots.len(), 1);
        assert_eq!(abandoned.snapshots[0].path, None);
        assert_eq!(abandoned.snapshots[0].format, Format::Html);
    }

    /// Nobody chose these files' permissions, so they get the private ones.
    #[test]
    fn snapshots_are_private_to_their_owner() {
        let scratch = Scratch::new("modes");
        let root = scratch.0.join("state").join("recovery");
        let (mut session, _) = Session::begin(&root).unwrap();
        let file = session.slot();
        write(&file, "first").unwrap();
        // Written again, over itself: the mode must survive the replacement.
        write(&file, "second").unwrap();

        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(file.parent().unwrap()), 0o700);
        assert_eq!(mode(&root), 0o700);
    }

    /// The write is interrupted after half the new snapshot is staged. What
    /// comes back is the previous snapshot, whole; the half is gone.
    #[test]
    fn a_crash_in_the_middle_of_a_write_keeps_the_previous_snapshot() {
        let scratch = Scratch::new("mid-write");
        let whole = render(None, Format::Markdown, SystemTime::now(), "<p>whole</p>");
        let file = crashed_with(&scratch.0, &whole);
        let half = temporary_for(&file);
        fs::write(&half, &whole.as_bytes()[..whole.len() / 2]).unwrap();

        let (_session, abandoned) = Session::begin(&scratch.0).unwrap();
        assert_eq!(abandoned.snapshots.len(), 1);
        assert_eq!(abandoned.snapshots[0].body, "<p>whole</p>");
        assert!(abandoned.unreadable.is_empty());
        assert!(!half.exists(), "the half-written file was left behind");
    }

    /// A first snapshot interrupted before its rename leaves nothing to
    /// offer, and nothing behind.
    #[test]
    fn a_crash_before_the_first_snapshot_lands_leaves_nothing() {
        let scratch = Scratch::new("first-write");
        let dir = {
            let (mut session, _) = Session::begin(&scratch.0).unwrap();
            let file = session.slot();
            fs::write(temporary_for(&file), b"pencil-recovery 1\nform").unwrap();
            file.parent().unwrap().to_path_buf()
        };

        let (_session, abandoned) = Session::begin(&scratch.0).unwrap();
        assert!(abandoned.snapshots.is_empty() && abandoned.unreadable.is_empty());
        assert!(!dir.exists(), "the dead window's empty directory was kept");
    }

    /// A write that fails leaves the snapshot that was there.
    #[test]
    fn a_failed_write_leaves_the_snapshot_that_was_there() {
        let scratch = Scratch::new("failed-write");
        let (mut session, _) = Session::begin(&scratch.0).unwrap();
        let file = session.slot();
        write(&file, "kept").unwrap();
        // The staging name is taken, so the write cannot begin.
        fs::write(temporary_for(&file), b"").unwrap();

        assert!(write(&file, "new").is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "kept");
    }

    /// A file that is where a snapshot would be and is not one is reported
    /// and left alone, and does not hide the ones that read.
    #[test]
    fn a_snapshot_that_will_not_read_is_reported_not_dropped() {
        let scratch = Scratch::new("unreadable");
        let broken = {
            let (mut session, _) = Session::begin(&scratch.0).unwrap();
            let good = session.slot();
            write(
                &good,
                &render(None, Format::Text, SystemTime::now(), "<p>good</p>"),
            )
            .unwrap();
            let broken = session.slot();
            write(&broken, "pencil-recovery 1\nformat=md\n").unwrap();
            broken
        };

        let (_session, abandoned) = Session::begin(&scratch.0).unwrap();
        assert_eq!(abandoned.snapshots.len(), 1);
        assert_eq!(abandoned.unreadable.len(), 1);
        assert_eq!(abandoned.unreadable[0].0, broken);
        assert!(broken.exists());
    }

    /// The window next to this one is alive: what it keeps is not offered.
    #[test]
    fn a_live_windows_snapshots_are_left_alone() {
        let scratch = Scratch::new("live");
        let (mut other, _) = Session::begin(&scratch.0).unwrap();
        let file = other.slot();
        write(
            &file,
            &render(None, Format::Markdown, SystemTime::now(), "<p>theirs</p>"),
        )
        .unwrap();

        let (_session, abandoned) = Session::begin(&scratch.0).unwrap();
        assert!(abandoned.snapshots.is_empty());
        assert!(file.exists());
    }

    /// Two windows start after a crash. The first to look is the one that
    /// offers the documents; the second does not offer them again.
    #[test]
    fn a_dead_windows_snapshots_are_offered_by_one_window_only() {
        let scratch = Scratch::new("claimed");
        crashed_with(
            &scratch.0,
            &render(None, Format::Markdown, SystemTime::now(), "<p>lost</p>"),
        );

        let (_first, offered) = Session::begin(&scratch.0).unwrap();
        let (_second, again) = Session::begin(&scratch.0).unwrap();
        assert_eq!(offered.snapshots.len(), 1);
        assert!(again.snapshots.is_empty());
    }

    /// An offer nobody answered is made again: the documents are still there
    /// for the window that starts after this one has gone.
    #[test]
    fn an_undecided_snapshot_is_offered_again_next_time() {
        let scratch = Scratch::new("later");
        crashed_with(
            &scratch.0,
            &render(None, Format::Markdown, SystemTime::now(), "<p>lost</p>"),
        );
        {
            let (first, offered) = Session::begin(&scratch.0).unwrap();
            assert_eq!(offered.snapshots.len(), 1);
            first.end().unwrap();
        }
        let (_second, offered) = Session::begin(&scratch.0).unwrap();
        assert_eq!(offered.snapshots.len(), 1);
    }

    /// Restoring a document moves its snapshot into this window's session;
    /// discarding one removes it. Either way the dead window's directory goes
    /// once it is empty, and a clean exit leaves nothing at all.
    #[test]
    fn restoring_and_discarding_clear_what_the_dead_window_left() {
        let scratch = Scratch::new("cleared");
        let dead = {
            let (mut session, _) = Session::begin(&scratch.0).unwrap();
            for body in ["<p>one</p>", "<p>two</p>"] {
                let file = session.slot();
                write(
                    &file,
                    &render(None, Format::Markdown, SystemTime::now(), body),
                )
                .unwrap();
            }
            session.dir.clone()
        };

        let (mut session, abandoned) = Session::begin(&scratch.0).unwrap();
        assert_eq!(abandoned.snapshots.len(), 2);
        let adopted = session.adopt(&abandoned.snapshots[0].file).unwrap();
        assert_eq!(read(&adopted).unwrap().body, abandoned.snapshots[0].body);
        assert!(
            dead.exists(),
            "a directory with a snapshot left was removed"
        );

        discard(&abandoned.snapshots[1].file).unwrap();
        session.sweep();
        assert!(!dead.exists(), "the dead window's empty directory was kept");

        session.end().unwrap();
        let left: Vec<_> = fs::read_dir(&scratch.0)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from(LOCK)]);
    }

    /// A snapshot older than the file it is of says so: the file was saved,
    /// or changed elsewhere, after the snapshot was taken.
    #[test]
    fn a_snapshot_older_than_its_file_is_stale() {
        let scratch = Scratch::new("stale");
        fs::create_dir_all(&scratch.0).unwrap();
        let document = scratch.0.join("notes.md");
        fs::write(&document, "on disk").unwrap();
        let written = fs::metadata(&document).unwrap().modified().unwrap();

        let snapshot = |taken| Snapshot {
            file: scratch.0.join("1.snapshot"),
            path: Some(document.clone()),
            format: Format::Markdown,
            taken,
            body: String::new(),
        };
        assert!(snapshot(written - Duration::from_secs(60)).changed_since());
        assert!(!snapshot(written + Duration::from_secs(60)).changed_since());

        // A document whose file is gone, or that never had one, has nothing
        // newer to be compared with.
        fs::remove_file(&document).unwrap();
        assert!(!snapshot(written - Duration::from_secs(60)).changed_since());
    }

    /// Discarding twice, or discarding what a save already removed, is fine.
    #[test]
    fn discarding_a_snapshot_that_is_gone_is_not_an_error() {
        let scratch = Scratch::new("gone");
        fs::create_dir_all(&scratch.0).unwrap();
        assert!(discard(&scratch.0.join("9.snapshot")).is_ok());
    }
}
