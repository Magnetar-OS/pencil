// SPDX-License-Identifier: GPL-3.0-only

//! Putting a document on paper.
//!
//! # Why a PDF
//!
//! Because that is what the desktop's print portal takes. It hands a file
//! descriptor to whichever print dialog the session provides, and that dialog
//! speaks PDF. So printing is really two jobs: turn the document into a PDF,
//! and hand the PDF to the portal. This module does the first; [`crate::app`]
//! does the second.
//!
//! # Why the layout comes from cosmic-text
//!
//! Because it is the same engine that laid the document out on screen. Line
//! breaks, kerning, ligatures, bidirectional text and the fallback chain for a
//! character the chosen font does not have are all decisions that have to
//! match what the author saw while writing, and the only way to be sure they
//! match is to ask the same shaper. Every glyph on the page comes back from it
//! with a font, an id and a position; this module's work is to say where the
//! blocks go and to write the glyphs down.
//!
//! # What is not printed
//!
//! Syntax colouring, spelling squiggles, the caret and the selection: none of
//! those are the document. Images are not embedded yet — a block image prints
//! as its alt text.

use std::collections::{BTreeMap, HashMap};

use cosmic::iced::advanced::graphics::text::cosmic_text as ct;
use nib::blocks::{self, Block, Kind, Marker};
use nib_model::basic::{marks, nodes};
use nib_model::node::Node;
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, UnicodeCmap};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

/// The page a document is printed onto, in PostScript points.
#[derive(Debug, Clone, Copy)]
pub struct Sheet {
    pub width: f32,
    pub height: f32,
    pub margin: f32,
}

impl Default for Sheet {
    /// A4 with a two-centimetre margin.
    fn default() -> Self {
        Self {
            width: 595.28,
            height: 841.89,
            margin: 56.7,
        }
    }
}

impl Sheet {
    fn text_width(self) -> f32 {
        self.width - self.margin * 2.0
    }

}

/// Body text size, in points. Everything else is a multiple of it.
const BODY: f32 = 10.5;
/// How much taller a line is than the text on it.
const LEADING: f32 = 1.45;
/// One level of list or quote indentation.
const INDENT: f32 = 18.0;
/// The gap between two blocks.
const GAP: f32 = 6.0;

/// A count as a length. Nothing here counts past a few dozen, and going
/// through `u16` says so rather than letting a cast quietly round.
fn steps(n: usize) -> f32 {
    u16::try_from(n).unwrap_or(u16::MAX).into()
}

/// The heading sizes, largest first.
fn heading_size(level: i64) -> f32 {
    match level {
        1 => BODY * 1.9,
        2 => BODY * 1.55,
        3 => BODY * 1.3,
        4 => BODY * 1.15,
        _ => BODY * 1.05,
    }
}

/// One glyph, placed on the page.
struct Glyph {
    font: ct::fontdb::ID,
    weight: ct::fontdb::Weight,
    id: u16,
    /// Distance from the left edge of the page.
    x: f32,
    /// Distance from the bottom edge of the page, on the baseline.
    y: f32,
    size: f32,
    /// What the glyph came from, for the PDF's copy-and-paste map.
    text: String,
}

/// A rectangle of flat colour: a quote's bar, a code block's ground, a rule.
struct Fill {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    grey: f32,
}

/// One line of glyphs, before it is given a page.
struct Line {
    glyphs: Vec<Glyph>,
    /// How much vertical space the line occupies.
    height: f32,
    /// Where the baseline sits within that space, measured from its top.
    baseline: f32,
}

/// A page's worth of placed content.
#[derive(Default)]
struct Page {
    glyphs: Vec<Glyph>,
    fills: Vec<Fill>,
}

/// Renders a document to a PDF.
///
/// Never fails: a document with nothing printable in it produces one empty
/// page, which is what a printer would do with it anyway.
#[must_use]
pub fn to_pdf(doc: &Node, title: &str, sheet: Sheet) -> Vec<u8> {
    let mut fonts = ct::FontSystem::new();
    let pages = paginate(doc, &mut fonts, sheet);
    write_pdf(&pages, &mut fonts, title, sheet)
}

// -- layout -----------------------------------------------------------------

/// Lays the document out and cuts it into pages.
fn paginate(doc: &Node, fonts: &mut ct::FontSystem, sheet: Sheet) -> Vec<Page> {
    let blocks = blocks::flatten(doc);
    let mut pages = vec![Page::default()];
    // Measured down from the top of the page; flipped to PDF's bottom-up
    // coordinates only when a glyph is finally placed.
    let mut y = sheet.margin;
    let bottom = sheet.height - sheet.margin;

    let mut column_x: Vec<f32> = Vec::new();
    // A row's cells all start at its top and the row ends below the tallest of
    // them, so the two have to be tracked apart: writing the bottom back into
    // the top is how the second cell in a row ends up under the first.
    let mut row_top = 0.0_f32;
    let mut row_bottom = 0.0_f32;
    let mut row: Option<(usize, usize)> = None;

    for block in &blocks {
        let indent = sheet.margin
            + steps(block.indent + block.quote_depth) * INDENT;

        // A table cell is placed by its column rather than in the flow, and
        // the row only advances once every cell in it has been laid out.
        if let Some(cell) = block.cell {
            if row != Some((cell.table, cell.row)) {
                if row.is_some() {
                    y = row_bottom;
                }
                row = Some((cell.table, cell.row));
                row_top = y;
                row_bottom = y;
                column_x = columns(&blocks, cell.table, sheet);
                if cell.header {
                    grid(&mut pages, &column_x, sheet, y);
                }
            }
            let width = column_x
                .get(cell.column + 1)
                .map_or(sheet.margin + sheet.text_width(), |x| *x)
                - column_x.get(cell.column).copied().unwrap_or(sheet.margin);
            let x = column_x.get(cell.column).copied().unwrap_or(sheet.margin);
            let lines = lay_out(block, fonts, width - 8.0, cell.header);
            let mut cell_y = row_top;
            for line in lines {
                place(&mut pages, &mut cell_y, bottom, sheet, x + 4.0, line);
            }
            row_bottom = row_bottom.max(cell_y);
            y = row_bottom;
            continue;
        }
        if row.is_some() {
            row = None;
            y = row_bottom;
            grid(&mut pages, &column_x, sheet, y + 2.0);
            y += GAP;
        }

        match &block.kind {
            Kind::Rule => {
                if y + GAP * 2.0 > bottom {
                    pages.push(Page::default());
                    y = sheet.margin;
                }
                y += GAP;
                let page = pages.last_mut().expect("a page");
                page.fills.push(Fill {
                    x: sheet.margin,
                    y: sheet.height - y,
                    width: sheet.text_width(),
                    height: 0.6,
                    grey: 0.7,
                });
                y += GAP;
            }
            Kind::Atom { name } => {
                // No image is embedded yet, so what prints is the alt text —
                // which is what it is for.
                let text = block.text.clone();
                let label = if text.trim().is_empty() {
                    format!("[{name}]")
                } else {
                    format!("[{name}: {text}]")
                };
                for line in lay_out_text(&label, fonts, sheet.text_width(), BODY, false, true) {
                    place(&mut pages, &mut y, bottom, sheet, indent, line);
                }
                y += GAP;
            }
            Kind::Text => {
                let code = block.type_name == nodes::CODE_BLOCK;
                let lines = lay_out(block, fonts, sheet.margin + sheet.text_width() - indent, false);
                let start_page = pages.len();
                let start_y = y;
                for line in lines {
                    place(&mut pages, &mut y, bottom, sheet, indent, line);
                }
                decorate(&mut pages, block, sheet, indent, start_page, start_y, y, code);
                // List items breathe less than paragraphs do: a bulleted list
                // spaced like prose reads as several lists.
                y += if block.indent > 0 { GAP * 0.4 } else { GAP };
            }
        }
    }
    pages
}

/// Puts one line on the current page, starting a new one if it will not fit.
fn place(pages: &mut Vec<Page>, y: &mut f32, bottom: f32, sheet: Sheet, x: f32, line: Line) {
    if *y + line.height > bottom && !line.glyphs.is_empty() {
        pages.push(Page::default());
        *y = sheet.margin;
    }
    let baseline = sheet.height - (*y + line.baseline);
    let page = pages.last_mut().expect("a page");
    for mut glyph in line.glyphs {
        glyph.x += x;
        glyph.y = baseline - glyph.y;
        page.glyphs.push(glyph);
    }
    *y += line.height;
}

/// The bar beside a quote and the ground behind a code block.
#[allow(clippy::too_many_arguments)]
fn decorate(
    pages: &mut [Page],
    block: &Block,
    sheet: Sheet,
    indent: f32,
    start_page: usize,
    start_y: f32,
    end_y: f32,
    code: bool,
) {
    if block.quote_depth == 0 && !code {
        return;
    }
    // Only the part that landed on the page the block started on: a decoration
    // that spans a page break is a decoration drawn in two pieces, and one
    // piece is enough to say what the block is.
    let height = if pages.len() > start_page {
        sheet.height - sheet.margin - start_y
    } else {
        end_y - start_y
    };
    let Some(page) = pages.get_mut(start_page - 1) else {
        return;
    };
    if height <= 0.0 {
        return;
    }
    if code {
        page.fills.push(Fill {
            x: indent - 4.0,
            y: sheet.height - start_y - height - 2.0,
            width: sheet.margin + sheet.text_width() - indent + 8.0,
            height: height + 4.0,
            grey: 0.94,
        });
    }
    for depth in 0..block.quote_depth {
        page.fills.push(Fill {
            x: sheet.margin + steps(depth) * INDENT,
            y: sheet.height - start_y - height,
            width: 2.0,
            height,
            grey: 0.75,
        });
    }
}

/// A rule across the width of a table, under its header and along its foot.
fn grid(pages: &mut [Page], columns: &[f32], sheet: Sheet, y: f32) {
    let (Some(first), Some(last)) = (columns.first(), columns.last()) else {
        return;
    };
    let Some(page) = pages.last_mut() else {
        return;
    };
    page.fills.push(Fill {
        x: *first,
        y: sheet.height - y,
        width: last - first,
        height: 0.5,
        grey: 0.7,
    });
}

/// Where each of a table's columns starts.
fn columns(blocks: &[Block], table: usize, sheet: Sheet) -> Vec<f32> {
    let count = blocks
        .iter()
        .filter_map(|b| b.cell)
        .filter(|c| c.table == table)
        .map(|c| c.column + c.span)
        .max()
        .unwrap_or(1)
        .max(1);
    // Equal widths. Measuring the content to size the columns is what a
    // browser does; a printed table that is readable is enough here.
    let width = sheet.text_width() / steps(count);
    (0..=count)
        .map(|i| sheet.margin + steps(i) * width)
        .collect()
}

/// Shapes one block into lines.
fn lay_out(block: &Block, fonts: &mut ct::FontSystem, width: f32, force_bold: bool) -> Vec<Line> {
    let code = block.type_name == nodes::CODE_BLOCK;
    let size = block
        .level
        .map_or(if code { BODY * 0.95 } else { BODY }, heading_size);
    let heading = block.level.is_some() || force_bold;

    let prefix = block.marker.as_ref().map_or(String::new(), |marker| match marker {
        Marker::Bullet => "\u{2022}  ".to_owned(),
        Marker::Number(n) => format!("{n}.  "),
        Marker::Check(true) => "\u{2611}  ".to_owned(),
        Marker::Check(false) => "\u{2610}  ".to_owned(),
    });

    // The runs the block's marks divide it into, as byte ranges over `text`.
    let mut runs: Vec<(String, bool, bool, bool)> = Vec::new();
    if !prefix.is_empty() {
        runs.push((prefix, heading, false, false));
    }
    for (node, segment) in block.inline.iter().zip(&block.segments) {
        let text = block
            .text
            .get(segment.text_from..segment.text_to)
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let has = |name: &str| node.marks().iter().any(|m| m.typ().name() == name);
        runs.push((
            text.to_owned(),
            heading || has(marks::STRONG),
            has(marks::EM),
            code || has(marks::CODE),
        ));
    }
    if runs.is_empty() {
        // An empty block still takes a line's worth of paper.
        return vec![Line {
            glyphs: Vec::new(),
            height: size * LEADING,
            baseline: size * LEADING * 0.8,
        }];
    }
    shape(&runs, fonts, width, size)
}

/// Shapes a single unstyled string, for the labels this module makes up.
fn lay_out_text(
    text: &str,
    fonts: &mut ct::FontSystem,
    width: f32,
    size: f32,
    bold: bool,
    italic: bool,
) -> Vec<Line> {
    shape(&[(text.to_owned(), bold, italic, false)], fonts, width, size)
}

/// The one place cosmic-text is asked anything.
fn shape(
    runs: &[(String, bool, bool, bool)],
    fonts: &mut ct::FontSystem,
    width: f32,
    size: f32,
) -> Vec<Line> {
    let metrics = ct::Metrics::new(size, size * LEADING);
    let mut buffer = ct::Buffer::new(fonts, metrics);
    let mut buffer = buffer.borrow_with(fonts);
    buffer.set_size(Some(width.max(1.0)), None);
    buffer.set_wrap(ct::Wrap::WordOrGlyph);

    let spans = runs.iter().map(|(text, bold, italic, mono)| {
        let mut attrs = ct::Attrs::new().family(if *mono {
            ct::Family::Monospace
        } else {
            ct::Family::SansSerif
        });
        if *bold {
            attrs = attrs.weight(ct::Weight::BOLD);
        }
        if *italic {
            attrs = attrs.style(ct::Style::Italic);
        }
        (text.as_str(), attrs)
    });
    buffer.set_rich_text(
        spans,
        &ct::Attrs::new(),
        ct::Shaping::Advanced,
        Some(ct::Align::Left),
    );

    let source: String = runs.iter().map(|(text, ..)| text.as_str()).collect();
    let mut lines = Vec::new();
    for run in buffer.layout_runs() {
        let height = run.line_height;
        let glyphs = run
            .glyphs
            .iter()
            .map(|glyph| Glyph {
                font: glyph.font_id,
                weight: glyph.font_weight,
                id: glyph.glyph_id,
                x: glyph.x + glyph.x_offset,
                // Relative to the line's baseline, positive downwards; `place`
                // turns it into a page coordinate.
                y: glyph.y_offset,
                size: glyph.font_size,
                text: source
                    .get(glyph.start..glyph.end)
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect();
        lines.push(Line {
            glyphs,
            height,
            baseline: (run.line_y - run.line_top).clamp(0.0, height),
        });
    }
    if lines.is_empty() {
        lines.push(Line {
            glyphs: Vec::new(),
            height: size * LEADING,
            baseline: size * LEADING * 0.8,
        });
    }
    lines
}

// -- the PDF itself ---------------------------------------------------------

/// What one embedded font needs written about it.
struct Embedded {
    reference: Ref,
    descendant: Ref,
    descriptor: Ref,
    file: Ref,
    cmap: Ref,
    name: String,
    /// Every glyph used, and the text it stood for.
    glyphs: BTreeMap<u16, String>,
}

fn write_pdf(pages: &[Page], fonts: &mut ct::FontSystem, title: &str, sheet: Sheet) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let mut next = 1;
    let mut alloc = || {
        let id = Ref::new(next);
        next += 1;
        id
    };

    let catalog = alloc();
    let tree = alloc();

    // Every font used, with the glyphs each was asked for.
    let mut used: HashMap<(ct::fontdb::ID, u16), Embedded> = HashMap::new();
    for page in pages {
        for glyph in &page.glyphs {
            let key = (glyph.font, glyph.weight.0);
            let entry = used.entry(key).or_insert_with(|| Embedded {
                reference: alloc(),
                descendant: alloc(),
                descriptor: alloc(),
                file: alloc(),
                cmap: alloc(),
                name: format!("F{}", used_name(key)),
                glyphs: BTreeMap::new(),
            });
            entry
                .glyphs
                .entry(glyph.id)
                .or_insert_with(|| glyph.text.clone());
        }
    }

    let page_ids: Vec<Ref> = pages.iter().map(|_| alloc()).collect();
    let content_ids: Vec<Ref> = pages.iter().map(|_| alloc()).collect();

    pdf.catalog(catalog).pages(tree);
    let mut tree_writer = pdf.pages(tree);
    tree_writer.count(i32::try_from(pages.len()).unwrap_or(0));
    tree_writer.kids(page_ids.iter().copied());
    tree_writer.finish();

    let media = Rect::new(0.0, 0.0, sheet.width, sheet.height);
    for (index, page) in pages.iter().enumerate() {
        let mut writer = pdf.page(page_ids[index]);
        writer.parent(tree).media_box(media).contents(content_ids[index]);
        let mut resources = writer.resources();
        let mut fonts_dict = resources.fonts();
        for embedded in used.values() {
            fonts_dict.pair(
                Name(embedded.name.as_bytes()),
                embedded.reference,
            );
        }
        fonts_dict.finish();
        resources.finish();
        writer.finish();

        let content = draw(page, &used);
        pdf.stream(content_ids[index], &content).filter(Filter::FlateDecode);
    }

    for ((id, weight), embedded) in &used {
        embed(&mut pdf, fonts, *id, ct::fontdb::Weight(*weight), embedded);
    }

    let mut info = pdf.document_info(alloc());
    info.title(TextStr(title));
    info.producer(TextStr("Pencil"));
    info.finish();

    pdf.finish()
}

/// A stable, unique PDF resource name for a font.
fn used_name(key: (ct::fontdb::ID, u16)) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&format!("{:?}-{}", key.0, key.1), &mut hasher);
    format!("{:x}", std::hash::Hasher::finish(&hasher))
}

/// One page's content stream.
fn draw(page: &Page, used: &HashMap<(ct::fontdb::ID, u16), Embedded>) -> Vec<u8> {
    let mut content = Content::new();

    for fill in &page.fills {
        content.save_state();
        content.set_fill_rgb(fill.grey, fill.grey, fill.grey);
        content.rect(fill.x, fill.y, fill.width, fill.height);
        content.fill_nonzero();
        content.restore_state();
    }
    if page.glyphs.is_empty() {
        return flate(&content.finish());
    }

    content.begin_text();
    content.set_fill_rgb(0.0, 0.0, 0.0);

    // Every glyph is placed by the shaper, so every glyph is written at the
    // position the shaper gave it. Running several into one string would mean
    // trusting the reader's width table to agree with cosmic-text about every
    // advance in between, and a word that drifts is worse than a stream that
    // is bigger before Flate gets to it.
    let mut current: Option<(&str, f32)> = None;
    for glyph in &page.glyphs {
        let Some(embedded) = used.get(&(glyph.font, glyph.weight.0)) else {
            continue;
        };
        if current != Some((embedded.name.as_str(), glyph.size)) {
            content.set_font(Name(embedded.name.as_bytes()), glyph.size);
            current = Some((embedded.name.as_str(), glyph.size));
        }
        content.set_text_matrix([1.0, 0.0, 0.0, 1.0, glyph.x, glyph.y]);
        content.show(Str(&glyph.id.to_be_bytes()));
    }
    content.end_text();
    flate(&content.finish())
}

fn flate(data: &[u8]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write as _;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_else(|_| data.to_vec())
}

/// Writes one font into the file.
fn embed(
    pdf: &mut Pdf,
    fonts: &mut ct::FontSystem,
    id: ct::fontdb::ID,
    weight: ct::fontdb::Weight,
    embedded: &Embedded,
) {
    let Some(font) = fonts.get_font(id, weight) else {
        return;
    };
    let data = font.data();
    let swash = font.as_swash();
    let metrics = swash.metrics(&[]);
    let units = f32::from(metrics.units_per_em).max(1.0);
    let scale = 1000.0 / units;
    let widths = swash.glyph_metrics(&[]);

    let base = format!("Pencil{}", embedded.name);
    let base = Name(base.as_bytes());

    // The Type 0 wrapper: two bytes per glyph, straight through to glyph ids.
    let mut type0 = pdf.type0_font(embedded.reference);
    type0
        .base_font(base)
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(embedded.descendant)
        .to_unicode(embedded.cmap);
    type0.finish();

    let mut cid = pdf.cid_font(embedded.descendant);
    cid.subtype(CidFontType::Type2)
        .base_font(base)
        .system_info(SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        })
        .font_descriptor(embedded.descriptor)
        .default_width(0.0)
        .cid_to_gid_map_predefined(Name(b"Identity"));
    let mut w = cid.widths();
    for id in embedded.glyphs.keys() {
        let advance = widths.advance_width(swash::GlyphId::from(*id)) * scale;
        w.same(*id, *id, advance);
    }
    w.finish();
    cid.finish();

    let slant = match swash.attributes().style() {
        swash::Style::Normal => 0.0,
        swash::Style::Italic => -12.0,
        swash::Style::Oblique(angle) => -angle.to_degrees(),
    };
    let mut flags = FontFlags::SYMBOLIC;
    if metrics.is_monospace {
        flags |= FontFlags::FIXED_PITCH;
    }
    if slant != 0.0 {
        flags |= FontFlags::ITALIC;
    }
    let mut descriptor = pdf.font_descriptor(embedded.descriptor);
    descriptor
        .name(base)
        .flags(flags)
        .bbox(Rect::new(
            -0.5 * 1000.0,
            metrics.descent * scale,
            1.5 * 1000.0,
            metrics.ascent * scale,
        ))
        .italic_angle(slant)
        .ascent(metrics.ascent * scale)
        .descent(-metrics.descent.abs() * scale)
        .cap_height(if metrics.cap_height > 0.0 {
            metrics.cap_height * scale
        } else {
            metrics.ascent * scale
        })
        // Not measured: no reader lays text out from it, and the alternative
        // is scanning the outlines of a stem to find out how thick it is.
        .stem_v(80.0);
    descriptor.font_file2(embedded.file);
    descriptor.finish();

    let compressed = flate(data);
    let mut file = pdf.stream(embedded.file, &compressed);
    file.filter(Filter::FlateDecode);
    file.pair(
        Name(b"Length1"),
        i32::try_from(data.len()).unwrap_or(i32::MAX),
    );
    file.finish();

    // The map back from glyph to text, so the PDF can be searched and copied.
    let mut cmap = UnicodeCmap::new(Name(b"Custom"), SystemInfo {
        registry: Str(b"Adobe"),
        ordering: Str(b"Identity"),
        supplement: 0,
    });
    for (id, text) in &embedded.glyphs {
        if !text.is_empty() {
            cmap.pair_with_multiple(*id, text.chars());
        }
    }
    pdf.cmap(embedded.cmap, &cmap.finish());
}

// -- handing it to the desktop ----------------------------------------------

/// Opens the desktop's print dialog on a rendered PDF.
///
/// The portal takes a file descriptor rather than a path, so that a sandboxed
/// application can only print what it already has open. The file behind the
/// descriptor is unlinked as soon as it is opened: the portal reads through
/// the descriptor it was handed, and a temporary file left on disk after a
/// print is a temporary file nobody remembers to delete.
///
/// # Errors
///
/// The message to show when the file could not be written or the portal
/// refused. A dialog the user cancelled is not an error.
pub async fn send(pdf: Vec<u8>, title: &str) -> Result<(), String> {
    use ashpd::desktop::print::PrintProxy;

    let path = std::env::temp_dir().join(format!("pencil-{}.pdf", std::process::id()));
    tokio::fs::write(&path, pdf)
        .await
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let file = std::fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let _ = std::fs::remove_file(&path);

    let proxy = PrintProxy::new().await.map_err(|error| error.to_string())?;
    // No window identifier: getting one for a Wayland surface means exporting
    // the surface through the compositor, and the dialog is perfectly usable
    // unparented. No token either, which is what asks the portal to show its
    // own print dialog rather than reusing a previous answer.
    // A `Response` error is the user closing the dialog. Nothing happened,
    // which is what they asked for.
    let cancelled_or_failed = |error: ashpd::Error| match error {
        ashpd::Error::Response(_) => Ok(()),
        error => Err(error.to_string()),
    };
    match proxy.print(None, title, &file, None, true).await {
        Ok(request) => request.response().or_else(cancelled_or_failed),
        Err(error) => cancelled_or_failed(error),
    }
}
