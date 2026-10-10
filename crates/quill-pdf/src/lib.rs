//! PDF export from a computed `Layout`.
//!
//! Text uses the 14 standard PDF fonts, so nothing is embedded. Callers lay
//! the document out with *metric-compatible* fonts (Arial / Times New Roman /
//! Courier New, see [`Family::css`]); then every item's measured width equals
//! its width in the PDF font and each item can be placed at its layout x.
//! Images are embedded as raw RGB (with a soft mask for transparency).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use quill_layout::{Item, Layout, Line};

/// Pixels (CSS, 96/inch) to PDF points (72/inch).
const PT: f32 = 0.75;
/// Default text colour, matching the editor.
const TEXT_RGB: u32 = 0x1F2328;

/// The three standard-font families a document font is mapped onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    Sans,
    Serif,
    Mono,
}

impl Family {
    /// Classify a font name; `None` (the document default) is serif.
    pub fn of(font: Option<&str>) -> Family {
        let Some(name) = font else {
            return Family::Serif;
        };
        let n = name.to_ascii_lowercase();
        const MONO: [&str; 8] = [
            "courier",
            "consolas",
            "menlo",
            "monaco",
            "lucida console",
            "cascadia",
            "source code",
            "fira",
        ];
        const SERIF: [&str; 12] = [
            "times",
            "cambria",
            "georgia",
            "garamond",
            "palatino",
            "book antiqua",
            "century",
            "baskerville",
            "bookman",
            "constantia",
            "minion",
            "sitka",
        ];
        if MONO.iter().any(|p| n.starts_with(p)) {
            Family::Mono
        } else if SERIF.iter().any(|p| n.starts_with(p)) {
            Family::Serif
        } else {
            Family::Sans
        }
    }

    /// The metric-compatible system font to lay text out with.
    pub fn css(self) -> &'static str {
        match self {
            Family::Sans => "Arial",
            Family::Serif => "Times New Roman",
            Family::Mono => "Courier New",
        }
    }

    /// Width of a space in 1/1000 em.
    fn space(self) -> f32 {
        match self {
            Family::Sans => 278.0,
            Family::Serif => 250.0,
            Family::Mono => 600.0,
        }
    }

    fn base_font(self, bold: bool, italic: bool) -> &'static str {
        match (self, bold, italic) {
            (Family::Sans, false, false) => "Helvetica",
            (Family::Sans, true, false) => "Helvetica-Bold",
            (Family::Sans, false, true) => "Helvetica-Oblique",
            (Family::Sans, true, true) => "Helvetica-BoldOblique",
            (Family::Serif, false, false) => "Times-Roman",
            (Family::Serif, true, false) => "Times-Bold",
            (Family::Serif, false, true) => "Times-Italic",
            (Family::Serif, true, true) => "Times-BoldItalic",
            (Family::Mono, false, false) => "Courier",
            (Family::Mono, true, false) => "Courier-Bold",
            (Family::Mono, false, true) => "Courier-Oblique",
            (Family::Mono, true, true) => "Courier-BoldOblique",
        }
    }
}

/// Decoded pixels for one image, 8-bit RGBA.
pub struct PdfImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Options {
    pub title: String,
    /// Deflate content streams and images (turn off to inspect output).
    pub compress: bool,
}

/// Map a character to its WinAnsi (Windows-1252) byte, if it has one.
fn win_ansi(c: char) -> Option<u8> {
    let b = match c {
        ' '..='~' | '\u{A0}'..='\u{FF}' => c as u8,
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        _ => return None,
    };
    Some(b)
}

/// A PDF literal string for `text`; characters outside WinAnsi become `?`.
fn literal(text: &str) -> String {
    let mut s = String::from("(");
    for c in text.chars() {
        let b = match c {
            '\t' | '\u{A0}' => b' ',
            '\n' | '\r' => continue,
            c => win_ansi(c).unwrap_or(b'?'),
        };
        match b {
            b'(' | b')' | b'\\' => {
                s.push('\\');
                s.push(b as char);
            }
            0x20..=0x7E => s.push(b as char),
            _ => {
                let _ = write!(s, "\\{b:03o}");
            }
        }
    }
    s.push(')');
    s
}

/// Compact decimal: at most 3 places, no trailing zeros.
fn num(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn rgb(c: u32) -> String {
    let ch = |shift: u32| num(((c >> shift) & 0xFF) as f32 / 255.0);
    format!("{} {} {}", ch(16), ch(8), ch(0))
}

/// UTF-16BE with a byte-order mark, as a PDF hex string.
fn utf16_hex(s: &str) -> String {
    let mut out = String::from("<FEFF");
    for u in s.encode_utf16() {
        let _ = write!(out, "{u:04X}");
    }
    out.push('>');
    out
}

struct Resources {
    /// (family, bold, italic) → resource name index.
    fonts: BTreeMap<(Family, bool, bool), usize>,
    /// image id → resource name index.
    images: BTreeMap<u32, usize>,
}

impl Resources {
    fn font(&mut self, family: Family, bold: bool, italic: bool) -> usize {
        let next = self.fonts.len() + 1;
        *self.fonts.entry((family, bold, italic)).or_insert(next)
    }

    fn image(&mut self, id: u32) -> usize {
        let next = self.images.len() + 1;
        *self.images.entry(id).or_insert(next)
    }
}

struct Page<'a> {
    height_pt: f32,
    layout: &'a Layout,
    images: &'a HashMap<u32, PdfImage>,
}

impl Page<'_> {
    fn y(&self, px: f32) -> f32 {
        self.height_pt - px * PT
    }

    fn text(&self, out: &mut String, res: &mut Resources, it: &Item, baseline_px: f32) {
        if it.text.is_empty() {
            return;
        }
        let family = Family::of(it.font.as_deref());
        let font = res.font(family, it.bold, it.italic);
        let color = rgb(it.color.unwrap_or(TEXT_RGB));
        let _ = writeln!(
            out,
            "{color} rg BT /F{font} {} Tf 1 0 0 1 {} {} Tm {} Tj ET",
            num(it.size * PT),
            num(it.x * PT),
            num(self.y(baseline_px)),
            literal(&it.text)
        );
        if it.underline {
            // Trailing spaces are not underlined.
            let spaces = it
                .text
                .chars()
                .rev()
                .take_while(|c| c.is_whitespace())
                .count() as f32;
            let w = (it.w - spaces * family.space() * it.size / 1000.0).max(0.0);
            let _ = writeln!(
                out,
                "{color} rg {} {} {} {} re f",
                num(it.x * PT),
                num(self.y(baseline_px + 2.0)),
                num(w * PT),
                num(PT)
            );
        }
    }

    fn line(&self, out: &mut String, res: &mut Resources, line: &Line) {
        let baseline = line.y + line.baseline;
        if let Some(m) = &line.marker {
            // The marker is drawn in the first text item's style, as on screen.
            let first = line.items.iter().find(|i| i.image.is_none());
            let (size, bold, font) = first.map_or((line.baseline, false, None), |i| {
                (i.size, i.bold, i.font.clone())
            });
            let marker = Item {
                x: m.x,
                text: m.text.clone(),
                bold,
                italic: false,
                underline: false,
                size,
                font,
                color: None,
                w: 0.0,
                start: 0,
                image: None,
                atomic: false,
            };
            self.text(out, res, &marker, baseline);
        }
        for it in &line.items {
            match it.image {
                Some(b) => {
                    if self.images.contains_key(&b.id) {
                        let n = res.image(b.id);
                        let _ = writeln!(
                            out,
                            "q {} 0 0 {} {} {} cm /Im{n} Do Q",
                            num(b.w * PT),
                            num(b.h * PT),
                            num(it.x * PT),
                            num(self.y(baseline))
                        );
                    }
                }
                None => self.text(out, res, it, baseline),
            }
        }
    }

    fn content(&self, page: usize, res: &mut Resources) -> String {
        let mut out = String::new();
        // Table cell borders.
        let cells: Vec<_> = self
            .layout
            .cells
            .iter()
            .filter(|c| c.page == page)
            .collect();
        if !cells.is_empty() {
            let _ = writeln!(out, "0.6 G 0.75 w");
            for c in cells {
                let _ = writeln!(
                    out,
                    "{} {} {} {} re S",
                    num(c.x * PT),
                    num(self.y(c.y + c.h)),
                    num(c.w * PT),
                    num(c.h * PT)
                );
            }
        }
        for line in self.layout.lines.iter().filter(|l| l.page == page) {
            self.line(&mut out, res, line);
        }
        out
    }
}

/// An in-progress PDF file: objects are appended and the cross-reference
/// table is built from their recorded offsets.
struct Pdf {
    buf: Vec<u8>,
    offsets: Vec<usize>,
    compress: bool,
}

impl Pdf {
    fn new(compress: bool) -> Pdf {
        Pdf {
            buf: b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            offsets: vec![],
            compress,
        }
    }

    /// Reserve the next object number.
    fn next_id(&self) -> usize {
        self.offsets.len() + 1
    }

    fn object(&mut self, body: &str) -> usize {
        let id = self.next_id();
        self.offsets.push(self.buf.len());
        self.buf
            .extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
        id
    }

    /// An object with a stream; `dict` holds the extra dictionary entries.
    fn stream(&mut self, dict: &str, data: &[u8]) -> usize {
        let (filter, body) = if self.compress {
            (
                " /Filter /FlateDecode",
                miniz_oxide::deflate::compress_to_vec_zlib(data, 6),
            )
        } else {
            ("", data.to_vec())
        };
        let id = self.next_id();
        self.offsets.push(self.buf.len());
        self.buf.extend_from_slice(
            format!(
                "{id} 0 obj\n<< {dict}{filter} /Length {} >>\nstream\n",
                body.len()
            )
            .as_bytes(),
        );
        self.buf.extend_from_slice(&body);
        self.buf.extend_from_slice(b"\nendstream\nendobj\n");
        id
    }

    fn finish(mut self, root: usize, info: usize) -> Vec<u8> {
        let xref = self.buf.len();
        let n = self.offsets.len() + 1;
        let mut table = format!("xref\n0 {n}\n0000000000 65535 f \n");
        for off in &self.offsets {
            let _ = writeln!(table, "{off:010} 00000 n ");
        }
        let _ = write!(
            table,
            "trailer\n<< /Size {n} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        );
        self.buf.extend_from_slice(table.as_bytes());
        self.buf
    }
}

/// Render every page of `layout`. `page_w`/`page_h` are in CSS px;
/// `images` maps document image ids to decoded pixels.
pub fn render(
    layout: &Layout,
    page_w: f32,
    page_h: f32,
    images: &HashMap<u32, PdfImage>,
    opts: &Options,
) -> Vec<u8> {
    let pager = Page {
        height_pt: page_h * PT,
        layout,
        images,
    };
    let mut res = Resources {
        fonts: BTreeMap::new(),
        images: BTreeMap::new(),
    };
    let contents: Vec<String> = (0..layout.pages)
        .map(|p| pager.content(p, &mut res))
        .collect();

    let mut pdf = Pdf::new(opts.compress);
    // Fixed object numbers for the catalog tree; pages follow the shared resources.
    let catalog = pdf.object("<< /Type /Catalog /Pages 2 0 R >>");
    debug_assert_eq!(catalog, 1);
    let pages_id = pdf.next_id(); // 2: patched below once the kids are known
    pdf.offsets.push(0);
    let info = pdf.object(&format!(
        "<< /Title {} /Producer (Quill) /Creator (Quill Write) >>",
        utf16_hex(&opts.title)
    ));

    let mut font_refs = String::new();
    for (&(family, bold, italic), &n) in &res.fonts {
        let id = pdf.object(&format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /{} /Encoding /WinAnsiEncoding >>",
            family.base_font(bold, italic)
        ));
        let _ = write!(font_refs, "/F{n} {id} 0 R ");
    }
    let mut image_refs = String::new();
    for (&image_id, &n) in &res.images {
        let img = &images[&image_id];
        let px = (img.width as usize) * (img.height as usize);
        let rgba = &img.rgba[..img.rgba.len().min(px * 4)];
        let mut color = Vec::with_capacity(px * 3);
        let mut alpha = Vec::with_capacity(px);
        for p in rgba.chunks_exact(4) {
            color.extend_from_slice(&p[..3]);
            alpha.push(p[3]);
        }
        let dims = format!(
            "/Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent 8",
            img.width, img.height
        );
        let mask = alpha
            .iter()
            .any(|&a| a != 255)
            .then(|| pdf.stream(&format!("{dims} /ColorSpace /DeviceGray"), &alpha));
        let smask = mask.map_or(String::new(), |m| format!(" /SMask {m} 0 R"));
        let id = pdf.stream(&format!("{dims} /ColorSpace /DeviceRGB{smask}"), &color);
        let _ = write!(image_refs, "/Im{n} {id} 0 R ");
    }

    let media = format!("[0 0 {} {}]", num(page_w * PT), num(page_h * PT));
    let mut kids = Vec::new();
    for content in &contents {
        let content_id = pdf.stream("", content.as_bytes());
        let page_id = pdf.object(&format!(
            "<< /Type /Page /Parent {pages_id} 0 R /MediaBox {media} /Contents {content_id} 0 R >>"
        ));
        kids.push(page_id);
    }
    // Now write the Pages node at its reserved slot (object 2) by appending and
    // fixing its offset: objects may be numbered in any file order.
    let kids_list: String = kids.iter().map(|k| format!("{k} 0 R ")).collect();
    let pages_body = format!(
        "<< /Type /Pages /Kids [{kids_list}] /Count {} /Resources << /Font << {font_refs}>> /XObject << {image_refs}>> >> >>",
        kids.len()
    );
    pdf.offsets[pages_id - 1] = pdf.buf.len();
    pdf.buf
        .extend_from_slice(format!("{pages_id} 0 obj\n{pages_body}\nendobj\n").as_bytes());
    pdf.finish(catalog, info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core::{
        Align, Cell, Document, Format, Image, Kind, Paragraph, Pos, Run, Story, PAGE_CHAR,
    };
    use quill_layout::{layout, PageSetup, TextStyle};

    // 6px per char at size 12 (scales with size).
    fn m(t: &str, s: &TextStyle) -> f32 {
        t.chars().count() as f32 * s.size / 2.0
    }

    const PAGE: PageSetup = PageSetup::uniform(400.0, 300.0, 40.0);

    fn pdf(doc: &Document, images: &HashMap<u32, PdfImage>, compress: bool) -> Vec<u8> {
        let l = layout(doc, PAGE, &m);
        render(
            &l,
            PAGE.width,
            PAGE.height,
            images,
            &Options {
                title: "Test".into(),
                compress,
            },
        )
    }

    fn text(bytes: &[u8]) -> String {
        // One char per byte (non-ASCII becomes '?'), so string offsets are byte offsets.
        bytes
            .iter()
            .map(|&b| if b < 128 { b as char } else { '?' })
            .collect()
    }

    /// Check the header, every xref offset, the trailer, and return the page count.
    fn validate(bytes: &[u8]) -> usize {
        let s = text(bytes);
        assert!(s.starts_with("%PDF-1.5\n") && s.trim_end().ends_with("%%EOF"));
        let sx = s.rfind("startxref\n").unwrap();
        let xref_at: usize = s[sx + 10..].lines().next().unwrap().parse().unwrap();
        assert!(s[xref_at..].starts_with("xref\n0 "));
        let mut lines = s[xref_at..].lines();
        lines.next();
        let n: usize = lines
            .next()
            .unwrap()
            .strip_prefix("0 ")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(lines.next().unwrap(), "0000000000 65535 f ");
        for id in 1..n {
            let entry = lines.next().unwrap();
            assert!(entry.ends_with(" 00000 n "), "{entry}");
            let off: usize = entry[..10].parse().unwrap();
            assert!(
                s[off..].starts_with(&format!("{id} 0 obj\n")),
                "object {id} not at offset {off}"
            );
        }
        assert!(s.contains(&format!("/Size {n} ")));
        let count = s
            .split("/Count ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        count.parse().unwrap()
    }

    fn doc(texts: &[&str]) -> Document {
        Document::from_paragraphs(texts.iter().map(|t| Paragraph::plain(t)).collect())
    }

    #[test]
    fn structure_text_and_fonts() {
        let bytes = pdf(&doc(&["Hello (world) \\ back"]), &HashMap::new(), false);
        assert_eq!(validate(&bytes), 1);
        let s = text(&bytes);
        assert!(
            s.contains("/MediaBox [0 0 300 225]"),
            "400x300 px = 300x225 pt"
        );
        assert!(
            s.contains("/BaseFont /Times-Roman"),
            "document default font is serif"
        );
        assert!(
            s.contains(r"(Hello ) Tj") && s.contains(r"(\(world\) ) Tj") && s.contains(r"(\\ ) Tj"),
            "{s}"
        );
        // 12pt = 16px → 12 pt font; text starts at the left margin 40px = 30pt, baseline 40px+16px from the top.
        assert!(s.contains("/F1 12 Tf 1 0 0 1 30 "), "{s}");
        assert!(s.contains("0.122 0.137 0.157 rg"), "default colour");
        assert!(s.contains("/Title <FEFF0054006500730074>"));
    }

    #[test]
    fn font_families_styles_colors_and_underline() {
        let mut d = doc(&[""]);
        let f = |font: &str, bold, italic| Format {
            font: Some(font.into()),
            bold,
            italic,
            ..Default::default()
        };
        d.paragraphs[0].runs = vec![
            Run::new("sans ", f("Calibri", false, false)),
            Run::new("bold ", f("Arial", true, false)),
            Run::new("ital ", f("Georgia", false, true)),
            Run::new("mono ", f("Consolas", true, true)),
            Run::new(
                "red",
                Format {
                    color: Some(0xFF0000),
                    underline: true,
                    ..Default::default()
                },
            ),
        ];
        let s = text(&pdf(&d, &HashMap::new(), false));
        for font in [
            "Helvetica",
            "Helvetica-Bold",
            "Times-Italic",
            "Courier-BoldOblique",
            "Times-Roman",
        ] {
            assert!(s.contains(&format!("/BaseFont /{font} ")), "{font}");
        }
        assert!(s.contains("1 0 0 rg"), "red text");
        assert!(s.contains(" re f"), "underline is a filled rectangle");
        assert_eq!(Family::of(Some("Segoe UI")), Family::Sans);
        assert_eq!(Family::of(Some("Cambria Math")), Family::Serif);
        assert_eq!(Family::of(Some("Fira Code")), Family::Mono);
        assert_eq!(Family::of(None), Family::Serif);
        assert_eq!(Family::Mono.css(), "Courier New");
    }

    #[test]
    fn winansi_encoding_and_unsupported_characters() {
        assert_eq!(
            literal("café – “x” • €"),
            "(caf\\351 \\226 \\223x\\224 \\225 \\200)"
        );
        assert_eq!(literal("日本"), "(??)", "outside WinAnsi becomes ?");
        assert_eq!(literal("a\tb\nc"), "(a bc)");
        assert_eq!(literal("\u{A0}"), "( )");
    }

    #[test]
    fn pages_headers_footers_and_page_numbers() {
        let mut d = doc(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);
        let mut footer = Paragraph::default();
        footer
            .runs
            .push(Run::new(format!("Page {PAGE_CHAR}"), Format::default()));
        d.set_story_paras(Story::Footer, vec![footer]);
        let bytes = pdf(&d, &HashMap::new(), false);
        let pages = validate(&bytes);
        assert!(pages >= 2, "{pages}");
        let s = text(&bytes);
        for p in 1..=pages {
            assert!(s.contains(&format!("({p}) Tj")), "page number {p} drawn");
        }
        assert_eq!(s.matches("/Type /Page ").count(), pages);
    }

    #[test]
    fn lists_markers_and_table_borders() {
        let mut ps = vec![
            Paragraph::plain("item").with_kind(Kind::Bullet),
            Paragraph::plain("one").with_kind(Kind::Numbered),
        ];
        for (c, t) in [(0u16, "A"), (1, "B")] {
            let mut p = Paragraph::plain(t);
            p.cell = Some(Cell {
                table: 0,
                row: 0,
                col: c,
            });
            ps.push(p);
        }
        let s = text(&pdf(&Document::from_paragraphs(ps), &HashMap::new(), false));
        assert!(s.contains("(\\225) Tj"), "bullet marker");
        assert!(s.contains("(1.) Tj"), "numbered marker");
        assert_eq!(s.matches(" re S").count(), 2, "one border box per cell");
        assert!(s.contains("0.6 G"));
    }

    #[test]
    fn images_are_embedded_with_soft_masks_only_when_transparent() {
        let mut d = doc(&["x"]);
        d.insert_image(
            Pos { para: 0, offset: 1 },
            Image::new("image/png", vec![1], 40, 20),
        );
        d.insert_image(
            Pos { para: 0, offset: 2 },
            Image::new("image/png", vec![2], 10, 10),
        );
        // image 0: 2x1 opaque; image 1: 1x1 half-transparent
        let mut imgs = HashMap::new();
        imgs.insert(
            0,
            PdfImage {
                width: 2,
                height: 1,
                rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
            },
        );
        imgs.insert(
            1,
            PdfImage {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 255, 128],
            },
        );
        let bytes = pdf(&d, &imgs, false);
        validate(&bytes);
        let s = text(&bytes);
        assert_eq!(
            s.matches("/Subtype /Image").count(),
            3,
            "two images + one soft mask"
        );
        assert_eq!(s.matches("/SMask").count(), 1);
        assert!(s.contains("/Width 2 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB"));
        assert!(
            s.contains("30 0 0 15 ") && s.contains("7.5 0 0 7.5 "),
            "40x20px and 10x10px in pt: {s}"
        );
        assert!(s.contains("/Im1 Do") && s.contains("/Im2 Do"));
        // An image whose pixels were not supplied is skipped, not fatal.
        let missing = text(&pdf(&d, &HashMap::new(), false));
        assert!(!missing.contains(" Do Q") && !missing.contains("/Subtype /Image"));
    }

    #[test]
    fn compression_shrinks_streams_and_keeps_the_file_valid() {
        let many = vec!["the quick brown fox jumps over the lazy dog"; 6];
        let d = doc(&many);
        let (plain, packed) = (
            pdf(&d, &HashMap::new(), false),
            pdf(&d, &HashMap::new(), true),
        );
        assert_eq!(validate(&plain), validate(&packed));
        assert!(packed.len() < plain.len());
        assert!(text(&packed).contains("/Filter /FlateDecode"));
    }

    #[test]
    fn alignment_and_empty_documents() {
        let mut d = doc(&["ab"]);
        d.paragraphs[0].align = Align::Right;
        let s = text(&pdf(&d, &HashMap::new(), false));
        // Right edge of the text area = 360px; "ab" is 2 x 8px = 16px wide → starts at
        // 344px = 258pt.
        assert!(s.contains("1 0 0 1 258 "), "{s}");
        let empty = pdf(&Document::default(), &HashMap::new(), true);
        assert_eq!(validate(&empty), 1);
    }
}
