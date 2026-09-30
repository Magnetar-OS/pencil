# Changelog

All notable changes to Pencil are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Unsaved changes are kept for recovery. Every ten seconds, and whenever the
  window loses focus, each document with unsaved changes is copied to a
  private folder (`~/.local/state/pencil/recovery`, readable by you alone).
  If Pencil crashes, is killed, or the power goes, the next launch lists what
  was left and offers each document back — Restore, Discard, or decide later.
  A restored document comes back unsaved, under its own name, with everything
  it held, including formatting its file format could not have saved. The
  copy is removed when the document is saved or closed, or its changes are
  discarded. Settings → Recovery turns it off.
- A file opened while Pencil is running — "Open With" in the file manager, or
  `magnetar-pencil notes.md` in a terminal — opens as a tab in the window that
  is already up, instead of starting a second window. Several files can be
  opened at once; each gets a tab.
- `magnetar-pencil --new-window` starts a second window, as the New window
  button does.

### Fixed

- Pencil can be started from launchers and file managers that use GIO (GNOME
  Files and its "Open With", `gio open`). The desktop entry declared D-Bus
  activation, which Pencil has never served, so those started nothing at all.

### Changed

- Rebuilt against the current COSMIC libraries (libcosmic `6af8b70`).
- **The package and the command are `magnetar-pencil`.** Other repositories
  ship an unrelated `pencil` (Evolus Pencil, a prototyping tool, 3.x) under
  that package and command name. Sharing it meant the repository pacman read
  first decided which program `pacman -S pencil` installed, the repository
  audit failed wherever both were enabled, and the two could not be installed
  together. The package replaces this project's own `pencil` 1.2.0 and earlier
  on upgrade and leaves Evolus Pencil alone. A terminal habit or a keyboard
  shortcut that runs `pencil` needs changing to `magnetar-pencil`; the
  applications menu entry and "Open With" are unchanged.

## [1.2.0] - 2026-09-29

### Added

- A Close folder button in the header while a folder is open. There was no
  way to leave a project once one was open.

### Changed

- Rebuilt against the current COSMIC libraries (libcosmic `03d7dcb`).
- Built against cosmic-ext-nib 1.2.0: links open on Ctrl+click, rich text copies as HTML, and Markdown keeps code, escapes, titles and nested lists through a save and reopen.
- Pencil appears once in the applications menu, under Office. It also claimed
  Utility, so COSMIC's app library showed it in two folders.

### Fixed

- `pencil notes.md` for a file that does not exist yet starts a document by
  that name, saved there on the first Save, instead of an untitled one. A
  relative path is remembered in Open Recent as the file it named.
- A print the desktop's print service fails is reported instead of passing
  silently as if the dialog had been cancelled, and two prints started in
  quick succession no longer share one temporary file.
- A settings change made in another Pencil window now takes effect here too:
  turning spell checking off or on, changing its language, learning a word, or
  changing the theme. Only the change arrived before; the dictionary and theme
  stayed as they were until a restart.
- Changing a setting in one window no longer writes that window's older copy
  of every other setting, so a file just opened in another window stays in
  Open Recent.
- Opening a file that is already open — from the Open dialog, Open Recent or
  the command line — switches to its tab instead of opening a second copy
  that would save over the first.
- Closing the window with unsaved documents asks about each one — Save,
  Discard or Cancel — instead of closing and losing them. This covers the
  close button and the desktop's close shortcut.
- A save that finishes after you switch tabs no longer marks the tab you
  switched to as saved, or gives it the saved file's path; a later Save there
  would have overwritten the other file.
- Text typed while a save is being written stays marked unsaved.
- Choosing "Save" in the close-tab dialog and then cancelling the file dialog
  no longer closes that tab, unsaved, the next time any other document is
  saved.
- A failed Save As leaves the document's name and format as they were.
- Saving keeps the file's permissions: a `0600` note was rewritten with the
  default `0644`. Saving a file opened through a symlink updates the file the
  link points at instead of replacing the link with a copy.

## [1.1.0] - 2026-09-22

### Changed

- Rebuilt against the current COSMIC libraries (libcosmic `03c8f93`).

## [1.0.2] - 2026-09-16

### Fixed

- Packages are built. The v1.0.1 release stopped at the pipeline's formatting
  check before producing any, so it has no downloads; this is the first
  Pencil release with packages.

## [1.0.1] - 2026-09-16

### Added

- Packages. Pencil is built as a `.deb`, `.rpm` and Arch package on every
  release and published to the `[magnetar]` pacman repository. v1.0.0 was
  tagged without a release pipeline, so this is the first version anyone can
  install without building it.

## [1.0.0] - 2026-09-10

First release.

### Added

- Opens and saves Markdown, MDC, MDX, HTML and plain text, as one document
  model rather than a text buffer.
- Formatting from the keyboard, a toolbar, or Markdown shortcuts as you type.
- Find and replace — loose, case-sensitive, whole-word or regular expression.
- A heading outline in the sidebar, tabs, a project sidebar with git status,
  and code blocks coloured by language.
- Spell checking with corrections in the right-click menu, the clipboard from
  that menu, printing, a second window, folder search and Vim keys.
- An unsaved-changes guard, zoom, recent files, appearance settings and
  document statistics.

[Unreleased]: https://github.com/Magnetar-OS/pencil/compare/v1.2.0...HEAD
[1.2.0]: https://github.com/Magnetar-OS/pencil/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/Magnetar-OS/pencil/compare/v1.0.2...v1.1.0
[1.0.2]: https://github.com/Magnetar-OS/pencil/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/Magnetar-OS/pencil/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/Magnetar-OS/pencil/releases/tag/v1.0.0
