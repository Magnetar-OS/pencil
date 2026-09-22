# Changelog

All notable changes to Pencil are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Pencil appears once in the applications menu, under Office. It also claimed
  Utility, so COSMIC's app library showed it in two folders.

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

[Unreleased]: https://github.com/Magnetar-OS/pencil/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/Magnetar-OS/pencil/compare/v1.0.2...v1.1.0
[1.0.2]: https://github.com/Magnetar-OS/pencil/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/Magnetar-OS/pencil/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/Magnetar-OS/pencil/releases/tag/v1.0.0
