# Pencil

A rich text editor for the COSMIC desktop.

Pencil edits a *document*, not a text buffer. Headings are headings, lists are
lists, and a document saved as HTML and reopened is the same document rather
than an approximation of one. That is not a claim about care taken; it is a
property of the engine underneath, and there are tests that hold it to it.

There is no web engine anywhere in it.

## What it does

- **Opens and saves** Markdown, MDC, MDX, HTML and plain text. Changing the
  extension changes the file, not the document.
- **Formats** from the keyboard, a toolbar, or Markdown shortcuts as you type:
  `# ` becomes a heading, `- ` a bullet, `**bold**` bold.
- **Finds and replaces**, matching loosely, by case, by whole word, or by
  regular expression.
- **Outlines** the document's headings in the sidebar, and jumps to them.
- **Colours code blocks** by language.
- **Follows the desktop** into dark mode, and the desktop's own text engine for
  layout — a word is the same width here as in every other COSMIC application.

## What each format keeps

| | Structure | Marks | Underline |
|---|---|---|---|
| HTML | everything | everything | yes |
| Markdown | everything | all but underline | no |
| MDC, MDX | everything, plus components | all but underline | no |
| Plain text | block shapes | none | no |

The status bar marks a format that cannot hold everything the document does
with a `•`, once, where the format is already named — rather than a dialog on
every save that nobody reads.

## Building

```
just build-release
just run
just check-all      # what CI runs, cheapest failure first
```

## The engine

Pencil is a front end for [Nib](https://github.com/Magnetar-OS/cosmic-ext-nib),
which is MPL-2.0 and linked rather than absorbed. Nib holds the document model,
the schema, the steps, the serialisers and the widget; Pencil is a file dialog,
a toolbar and a settings pane over it. That division is the point: an
application built on the engine is mostly chrome.

## Licence

GPL-3.0-only for this application. The engine is MPL-2.0 — see its
`LICENSING.md` for why the split exists.
