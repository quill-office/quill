//! `.docx` (OOXML WordprocessingML) reader/writer.
//!
//! Supported: paragraphs, runs with bold/italic/underline, tabs and line
//! breaks, Heading 1–3, bulleted and numbered lists (single level). Anything
//! else (tables, images, other styles, …) is dropped on import.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use quill_core::{
    Align, Cell, Defaults, Document, Format, Image, Kind, Page, Paragraph, Run, OBJECT_CHAR,
};

mod styles;
use styles::{apply_para_prop, apply_run_prop, PProps, RProps, Styles, Theme};

#[derive(Debug, thiserror::Error)]
pub enum DocxError {
    #[error("not a valid zip container: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("word/document.xml is missing")]
    MissingDocument,
    #[error("malformed XML: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

fn read_part(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<String>, DocxError> {
    match zip.by_name(name) {
        Ok(mut f) => {
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            Ok(Some(s))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn read(bytes: &[u8]) -> Result<Document, DocxError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let xml = read_part(&mut zip, "word/document.xml")?.ok_or(DocxError::MissingDocument)?;
    let numbering = match read_part(&mut zip, "word/numbering.xml")? {
        Some(n) => parse_numbering(&n)?,
        None => HashMap::new(),
    };
    let media = read_media(&mut zip)?;
    let theme = match read_part(&mut zip, "word/theme/theme1.xml")? {
        Some(t) => styles::parse_theme(&t)?,
        None => Theme::default(),
    };
    let styles = match read_part(&mut zip, "word/styles.xml")? {
        Some(x) => Styles::parse(&x, theme)?,
        None => Styles::default(),
    };
    parse_document_xml(&xml, &numbering, &media, &styles)
}

/// Embedded images by relationship id: `rId → (mime, bytes)`.
type Media = HashMap<String, (String, Vec<u8>)>;

fn mime_for(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None, // e.g. EMF/WMF/TIFF: not renderable in browsers
    }
}

fn read_media(zip: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Result<Media, DocxError> {
    let mut media = Media::new();
    let Some(rels) = read_part(zip, "word/_rels/document.xml.rels")? else {
        return Ok(media);
    };
    let mut r = Reader::from_str(&rels);
    loop {
        match r.read_event()? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "Relationship" => {
                let (Some(id), Some(target), Some(kind)) =
                    (attr(&e, "Id"), attr(&e, "Target"), attr(&e, "Type"))
                else {
                    continue;
                };
                if !kind.ends_with("/image")
                    || attr(&e, "TargetMode").as_deref() == Some("External")
                {
                    continue;
                }
                let Some(mime) = mime_for(&target) else {
                    continue;
                };
                let path = match target.strip_prefix('/') {
                    Some(abs) => abs.to_string(),
                    None => format!("word/{target}"),
                };
                if let Ok(mut f) = zip.by_name(&path) {
                    let mut data = Vec::new();
                    f.read_to_end(&mut data)?;
                    media.insert(id, (mime.to_string(), data));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(media)
}

pub(crate) fn attr(e: &BytesStart, local: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == local)
        .map(|a| a.value.to_string())
}

/// `<w:b/>`-style toggles are on unless `w:val` says otherwise.
pub(crate) fn toggle_on(e: &BytesStart) -> bool {
    !matches!(
        attr(e, "val").as_deref(),
        Some("false" | "0" | "off" | "none")
    )
}

/// numId → "is a bullet list", from `numbering.xml` (first level only).
fn parse_numbering(xml: &str) -> Result<HashMap<u32, bool>, DocxError> {
    let mut r = Reader::from_str(xml);
    let mut abstract_bullet: HashMap<String, bool> = HashMap::new();
    let mut num_abstract: HashMap<u32, String> = HashMap::new();
    let (mut cur_abs, mut cur_num, mut cur_lvl): (Option<String>, Option<u32>, Option<String>) =
        (None, None, None);
    loop {
        let ev = r.read_event()?;
        let (e, is_start) = match &ev {
            Event::Start(e) => (Some(e), true),
            Event::Empty(e) => (Some(e), false),
            Event::End(e) => {
                match e.local_name().as_ref() {
                    "abstractNum" => cur_abs = None,
                    "num" => cur_num = None,
                    "lvl" => cur_lvl = None,
                    _ => {}
                }
                (None, false)
            }
            Event::Eof => break,
            _ => (None, false),
        };
        let Some(e) = e else { continue };
        match e.local_name().as_ref() {
            "abstractNum" if is_start => cur_abs = attr(e, "abstractNumId"),
            "lvl" if is_start => cur_lvl = attr(e, "ilvl"),
            "numFmt" => {
                if let (Some(a), Some("0")) = (&cur_abs, cur_lvl.as_deref()) {
                    abstract_bullet.insert(a.clone(), attr(e, "val").as_deref() == Some("bullet"));
                }
            }
            "num" if is_start => cur_num = attr(e, "numId").and_then(|v| v.parse().ok()),
            "abstractNumId" => {
                if let (Some(n), Some(a)) = (cur_num, attr(e, "val")) {
                    num_abstract.insert(n, a);
                }
            }
            _ => {}
        }
    }
    Ok(num_abstract
        .into_iter()
        .map(|(n, a)| (n, abstract_bullet.get(&a).copied().unwrap_or(true)))
        .collect())
}

fn kind_from_style(id: &str) -> Option<Kind> {
    let lower = id.to_ascii_lowercase().replace(' ', "");
    if let Some(level) = lower.strip_prefix("heading") {
        let n: u8 = level.parse().ok()?;
        return Some(Kind::Heading(n.clamp(1, 3)));
    }
    match lower.as_str() {
        "title" => Some(Kind::Heading(1)),
        "listbullet" => Some(Kind::Bullet),
        "listnumber" => Some(Kind::Numbered),
        _ => None,
    }
}

/// What a paragraph's style layers add up to (before direct run formatting).
struct Resolved {
    r: RProps,
    p: PProps,
    kind: Kind,
}

/// Quill's own default size for a paragraph kind (half-points), or the
/// document default for body text.
fn kind_default_size(kind: Kind, defaults: &Defaults) -> u16 {
    match kind {
        Kind::Heading(1) => 40,
        Kind::Heading(2) => 32,
        Kind::Heading(_) => 28,
        _ => defaults.size.unwrap_or(24),
    }
}

/// Flatten resolved properties into a run `Format`, leaving out anything
/// Quill already implies (document defaults, heading size and weight).
fn run_format(
    res: &Resolved,
    styles: &Styles,
    run_style: Option<&str>,
    direct: &RProps,
    defaults: &Defaults,
) -> Format {
    let mut r = res.r.clone();
    if let Some(id) = run_style {
        r.overlay(&styles.char_props(id));
    }
    r.overlay(direct);
    let heading = matches!(res.kind, Kind::Heading(_));
    Format {
        bold: r.bold.unwrap_or(false) && !heading,
        italic: r.italic.unwrap_or(false),
        underline: r.underline.unwrap_or(false),
        image: None,
        size: r
            .size
            .filter(|&s| s != kind_default_size(res.kind, defaults)),
        font: r.font.filter(|f| defaults.font.as_ref() != Some(f)),
        color: r.color,
    }
}

fn parse_document_xml(
    xml: &str,
    numbering: &HashMap<u32, bool>,
    media: &Media,
    styles: &Styles,
) -> Result<Document, DocxError> {
    let mut r = Reader::from_str(xml);
    let defaults = styles.defaults();
    let theme = &styles.theme;
    let mut paras: Vec<Paragraph> = Vec::new();
    let mut cur: Option<Paragraph> = None;
    // Paragraph state: style layers, direct properties, and what they resolve to.
    let mut style_kind: Option<Kind> = None;
    let mut num_kind: Option<Kind> = None;
    let mut para_style: Option<String> = None;
    let mut direct_p = PProps::default();
    let mut in_ppr = false;
    let mut resolved: Option<Resolved> = None;
    // Run state.
    let mut direct_r = RProps::default();
    let mut run_style: Option<String> = None;
    let mut run_fmt: Option<Format> = None;
    let mut in_text = false;
    // Table state. Only the outermost table is modelled; paragraphs of nested
    // tables are folded into the enclosing cell.
    let (mut depth, mut tables, mut row, mut col) = (0u32, 0u32, -1i32, -1i32);
    let mut cell: Option<Cell> = None;
    let mut table_style: Option<String> = None;
    // Drawing state: display size (px) and the embedded image's relationship id.
    let mut images: Vec<Image> = Vec::new();
    let (mut in_drawing, mut extent, mut embed): (bool, Option<(u32, u32)>, Option<String>) =
        (false, None, None);
    let emu_to_px = |v: Option<String>| {
        v.and_then(|v| v.parse::<u64>().ok())
            .map(|v| (v / 9525) as u32)
    };
    let mut page = Page::default();

    // Resolve the current paragraph's style layers (once, when first needed).
    macro_rules! resolve {
        () => {{
            if resolved.is_none() {
                let (run_props, mut para_props) = styles.para_props(
                    if depth > 0 {
                        table_style.as_deref()
                    } else {
                        None
                    },
                    para_style.as_deref(),
                );
                para_props.overlay(&direct_p);
                resolved = Some(Resolved {
                    r: run_props,
                    p: para_props,
                    kind: num_kind.or(style_kind).unwrap_or(Kind::Normal),
                });
            }
            resolved.as_ref().expect("just resolved")
        }};
    }
    // The format for text added to the current run.
    macro_rules! fmt_now {
        () => {{
            if run_fmt.is_none() {
                let res = resolve!();
                run_fmt = Some(run_format(
                    res,
                    styles,
                    run_style.as_deref(),
                    &direct_r,
                    &defaults,
                ));
            }
            run_fmt.clone().expect("just computed")
        }};
    }
    // Build a finished paragraph from the resolved layers.
    macro_rules! finish {
        ($p:expr) => {{
            let res = resolve!();
            let list = res.kind.is_list();
            let p: &mut Paragraph = $p;
            p.cell = cell;
            p.kind = res.kind;
            p.align = res.p.align.unwrap_or(Align::Left);
            p.space_before = res.p.before;
            p.space_after = res.p.after;
            p.line_spacing = res.p.line;
            // List indents come from the numbering definition, which Quill models itself.
            p.indent_left = if list {
                None
            } else {
                res.p.left.filter(|&l| l > 0)
            };
            p.indent_first = if list {
                None
            } else {
                res.p.first.filter(|&f| f != 0)
            };
        }};
    }

    loop {
        match r.read_event()? {
            Event::Start(e) if e.local_name().as_ref() == "p" => {
                cur = Some(Paragraph::default());
                style_kind = None;
                num_kind = None;
                para_style = None;
                direct_p = PProps::default();
                in_ppr = false;
                resolved = None;
            }
            Event::Empty(e) if e.local_name().as_ref() == "p" => {
                // An empty paragraph still carries its style's spacing.
                style_kind = None;
                num_kind = None;
                para_style = None;
                direct_p = PProps::default();
                resolved = None;
                let mut p = Paragraph::default();
                finish!(&mut p);
                paras.push(p);
            }
            Event::Start(e) | Event::Empty(e) if is_prop_element(&e) => {
                if in_ppr {
                    // Paragraph properties; run properties in here are the paragraph
                    // mark's, which don't affect the text.
                    if cur.is_some() {
                        apply_para_prop(&e, &mut direct_p);
                    }
                } else if cur.is_some() && apply_run_prop(&e, &mut direct_r, theme) {
                    run_fmt = None;
                }
            }
            Event::Start(e) => match e.local_name().as_ref() {
                "drawing" => {
                    in_drawing = true;
                    extent = None;
                    embed = None;
                }
                "extent" if in_drawing => {
                    extent = emu_to_px(attr(&e, "cx")).zip(emu_to_px(attr(&e, "cy")));
                }
                "blip" if in_drawing => embed = attr(&e, "embed"),
                "pPr" => in_ppr = true,
                "tbl" => {
                    depth += 1;
                    if depth == 1 {
                        tables += 1;
                        row = -1;
                        table_style = None;
                    }
                }
                "tr" if depth == 1 => {
                    row += 1;
                    col = -1;
                }
                "tc" if depth == 1 => {
                    col += 1;
                    cell = Some(Cell {
                        table: tables - 1,
                        row: row.max(0) as u16,
                        col: col.max(0) as u16,
                    });
                }
                "r" => {
                    direct_r = RProps::default();
                    run_style = None;
                    run_fmt = None;
                }
                "pStyle" => {
                    para_style = attr(&e, "val");
                    style_kind = para_style.as_deref().and_then(kind_from_style);
                }
                "tblStyle" if depth == 1 => table_style = attr(&e, "val"),
                "rStyle" if !in_ppr => {
                    run_style = attr(&e, "val");
                    run_fmt = None;
                }
                "numId" => num_kind = numbering_kind(&e, numbering),
                "t" => in_text = true,
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "extent" if in_drawing => {
                    extent = emu_to_px(attr(&e, "cx")).zip(emu_to_px(attr(&e, "cy")));
                }
                "blip" if in_drawing => embed = attr(&e, "embed"),
                "pgSz" => {
                    let dim = |k: &str, d: u32| {
                        attr(&e, k)
                            .and_then(|v| v.parse().ok())
                            .filter(|&v| v > 0)
                            .unwrap_or(d)
                    };
                    page.width = dim("w", page.width);
                    page.height = dim("h", page.height);
                }
                "pgMar" => {
                    let m = |k: &str, d: u32| {
                        attr(&e, k)
                            .and_then(|v| v.parse::<i32>().ok())
                            .map_or(d, |v| v.max(0) as u32)
                    };
                    page.top = m("top", page.top);
                    page.right = m("right", page.right);
                    page.bottom = m("bottom", page.bottom);
                    page.left = m("left", page.left);
                }
                "pStyle" => {
                    para_style = attr(&e, "val");
                    style_kind = para_style.as_deref().and_then(kind_from_style);
                }
                "tblStyle" if depth == 1 => table_style = attr(&e, "val"),
                "rStyle" if !in_ppr => {
                    run_style = attr(&e, "val");
                    run_fmt = None;
                }
                "numId" => num_kind = numbering_kind(&e, numbering),
                "tab" => {
                    let f = fmt_now!();
                    push(&mut cur, "\t", &f);
                }
                "br" => {
                    let f = fmt_now!();
                    push(&mut cur, "\n", &f);
                }
                _ => {}
            },
            Event::Text(t) if in_text => {
                let f = fmt_now!();
                push(
                    &mut cur,
                    &t.xml_content(quick_xml::XmlVersion::Implicit1_0),
                    &f,
                );
            }
            Event::GeneralRef(g) if in_text => {
                let s = match g.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => "",
                };
                let f = fmt_now!();
                push(&mut cur, s, &f);
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "pPr" => in_ppr = false,
                "drawing" => {
                    in_drawing = false;
                    let found = embed.take().and_then(|rid| media.get(&rid));
                    if let (Some((mime, data)), Some(p)) = (found, cur.as_mut()) {
                        let (w, h) = extent.unwrap_or((200, 150));
                        p.runs.push(Run::new(
                            OBJECT_CHAR.to_string(),
                            Format {
                                image: Some(images.len() as u32),
                                ..Format::default()
                            },
                        ));
                        images.push(Image::new(mime.as_str(), data.clone(), w, h));
                    }
                }
                "tc" if depth == 1 => cell = None,
                "tbl" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        cell = None;
                        table_style = None;
                    }
                }
                "p" => {
                    if let Some(mut p) = cur.take() {
                        finish!(&mut p);
                        paras.push(p);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    let mut doc = Document::from_paragraphs(paras);
    doc.images = images;
    doc.page = page;
    // Quill's built-in body font stands for "no explicit default".
    doc.defaults = Defaults {
        font: defaults.font.filter(|f| &**f != DEFAULT_FONT),
        size: defaults.size.filter(|&s| s != 24),
    };
    Ok(doc)
}

/// Elements handled as run/paragraph properties (context decides which).
fn is_prop_element(e: &BytesStart) -> bool {
    matches!(
        e.local_name().as_ref(),
        "rFonts" | "sz" | "color" | "b" | "i" | "u" | "jc" | "spacing" | "ind"
    )
}

/// The font Quill's renderer uses when a document names none.
pub const DEFAULT_FONT: &str = "Georgia";

fn numbering_kind(e: &BytesStart, numbering: &HashMap<u32, bool>) -> Option<Kind> {
    let id: u32 = attr(e, "val")?.parse().ok()?;
    if id == 0 {
        return None; // numId 0 means "no list"
    }
    Some(if numbering.get(&id).copied().unwrap_or(true) {
        Kind::Bullet
    } else {
        Kind::Numbered
    })
}

fn push(cur: &mut Option<Paragraph>, text: &str, fmt: &Format) {
    if let Some(p) = cur {
        p.runs.push(Run::new(text, fmt.clone()));
    }
}

fn esc_attr(s: &str) -> String {
    esc(s).replace('"', "&quot;")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn run_xml(text: &str, f: &Format) -> String {
    // Child order follows the WordprocessingML schema (rFonts, b, i, color, sz, u).
    let mut props = String::new();
    if let Some(font) = &f.font {
        let n = esc_attr(font);
        props.push_str(&format!(
            "<w:rFonts w:ascii=\"{n}\" w:hAnsi=\"{n}\" w:cs=\"{n}\"/>"
        ));
    }
    if f.bold {
        props.push_str("<w:b/>");
    }
    if f.italic {
        props.push_str("<w:i/>");
    }
    if let Some(c) = f.color {
        props.push_str(&format!("<w:color w:val=\"{c:06X}\"/>"));
    }
    if let Some(sz) = f.size {
        props.push_str(&format!("<w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/>"));
    }
    if f.underline {
        props.push_str("<w:u w:val=\"single\"/>");
    }
    let rpr = if props.is_empty() {
        String::new()
    } else {
        format!("<w:rPr>{props}</w:rPr>")
    };
    let mut out = String::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut String| {
        if !buf.is_empty() {
            out.push_str(&format!(
                "<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                esc(buf)
            ));
            buf.clear();
        }
    };
    for c in text.chars() {
        match c {
            '\n' | '\t' => {
                flush(&mut buf, &mut out);
                out.push_str(&format!(
                    "<w:r>{rpr}<w:{}/></w:r>",
                    if c == '\n' { "br" } else { "tab" }
                ));
            }
            c => buf.push(c),
        }
    }
    flush(&mut buf, &mut out);
    out
}

const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>";

pub fn write(doc: &Document) -> Result<Vec<u8>, DocxError> {
    // Each contiguous run of numbered paragraphs gets its own list instance so
    // numbering restarts at 1, as it does in Quill.
    let mut body = String::new();
    let mut numbered_lists = 0u32;
    let mut prev_numbered = false;
    let mut prev_cell: Option<Cell> = None;
    let mut open_table: Option<(u32, u16, u16)> = None; // (table, row, col) currently open
    let ncols_of = |t: u32| doc.table_dims(t).map_or(1, |d| d.1);

    let mut media: Vec<(String, std::sync::Arc<[u8]>)> = Vec::new();
    let mut media_ids: HashMap<u32, (String, String)> = HashMap::new(); // image id → (rId, file name)
    let mut drawings = 0usize;

    for p in &doc.paragraphs {
        // Close cells/rows/tables as the paragraph's cell changes.
        let next = p.cell.map(|c| (c.table, c.row, c.col));
        if open_table != next {
            if let Some((t, r, c)) = open_table {
                match next {
                    Some((nt, nr, nc)) if nt == t && nr == r && nc != c => body.push_str("</w:tc>"),
                    Some((nt, nr, _)) if nt == t && nr != r => body.push_str("</w:tc></w:tr>"),
                    _ => body.push_str("</w:tc></w:tr></w:tbl>"),
                }
            }
            if let Some((t, r, c)) = next {
                let same_table = open_table.is_some_and(|o| o.0 == t);
                let new_row = !same_table || open_table.is_some_and(|o| o.1 != r);
                if !same_table {
                    let n = ncols_of(t);
                    body.push_str(
                        "<w:tbl><w:tblPr><w:tblW w:w=\"5000\" w:type=\"pct\"/><w:tblBorders>",
                    );
                    for side in ["top", "left", "bottom", "right", "insideH", "insideV"] {
                        body.push_str(&format!("<w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"808080\"/>"));
                    }
                    body.push_str("</w:tblBorders></w:tblPr><w:tblGrid>");
                    for _ in 0..n {
                        body.push_str(&format!("<w:gridCol w:w=\"{}\"/>", 9000 / n));
                    }
                    body.push_str("</w:tblGrid>");
                }
                if new_row {
                    body.push_str("<w:tr>");
                }
                let _ = c;
                body.push_str("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr>");
            }
            open_table = next;
        }
        if p.cell != prev_cell {
            prev_numbered = false;
            prev_cell = p.cell;
        }
        let style_and_num = match p.kind {
            Kind::Normal => String::new(),
            Kind::Heading(n) => format!("<w:pStyle w:val=\"Heading{n}\"/>"),
            Kind::Bullet => "<w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr>".to_string(),
            Kind::Numbered => {
                if !prev_numbered {
                    numbered_lists += 1;
                }
                format!("<w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{}\"/></w:numPr>", numbered_lists + 1)
            }
        };
        let jc = match p.align {
            Align::Left => "",
            Align::Center => "<w:jc w:val=\"center\"/>",
            Align::Right => "<w:jc w:val=\"right\"/>",
            Align::Justify => "<w:jc w:val=\"both\"/>",
        };
        let mut spacing = String::new();
        if p.space_before.is_some() || p.space_after.is_some() || p.line_spacing.is_some() {
            spacing.push_str("<w:spacing");
            if let Some(v) = p.space_before {
                spacing.push_str(&format!(" w:before=\"{v}\""));
            }
            if let Some(v) = p.space_after {
                spacing.push_str(&format!(" w:after=\"{v}\""));
            }
            if let Some(v) = p.line_spacing {
                spacing.push_str(&format!(" w:line=\"{v}\" w:lineRule=\"auto\""));
            }
            spacing.push_str("/>");
        }
        // List indents belong to the numbering definition.
        let mut ind = String::new();
        if !p.kind.is_list() && (p.indent_left.is_some() || p.indent_first.is_some()) {
            ind.push_str("<w:ind");
            if let Some(v) = p.indent_left {
                ind.push_str(&format!(" w:left=\"{v}\""));
            }
            match p.indent_first {
                Some(f) if f < 0 => ind.push_str(&format!(" w:hanging=\"{}\"", -(f as i32))),
                Some(f) => ind.push_str(&format!(" w:firstLine=\"{f}\"")),
                None => {}
            }
            ind.push_str("/>");
        }
        let ppr =
            if style_and_num.is_empty() && spacing.is_empty() && ind.is_empty() && jc.is_empty() {
                String::new()
            } else {
                format!("<w:pPr>{style_and_num}{spacing}{ind}{jc}</w:pPr>")
            };
        prev_numbered = p.kind == Kind::Numbered;
        body.push_str("<w:p>");
        body.push_str(&ppr);
        for r in &p.runs {
            let image = r
                .format
                .image
                .and_then(|id| doc.images.get(id as usize).map(|i| (id, i)));
            let ext = image.and_then(|(_, i)| ext_for(&i.mime));
            let (Some((id, img)), Some(ext)) = (image, ext) else {
                if r.format.image.is_none() {
                    body.push_str(&run_xml(&r.text, &r.format));
                }
                continue; // images we can't embed are dropped
            };
            let (rid, _) = media_ids.entry(id).or_insert_with(|| {
                let n = media.len() + 1;
                media.push((format!("image{n}.{ext}"), img.data.clone()));
                (format!("rId{}", n + 2), format!("image{n}.{ext}"))
            });
            for _ in r.text.chars() {
                drawings += 1;
                body.push_str(&drawing_xml(rid, drawings, img.width, img.height));
            }
        }
        body.push_str("</w:p>");
    }
    if open_table.is_some() {
        body.push_str("</w:tc></w:tr></w:tbl><w:p/>"); // Word requires a paragraph after a table
    }
    let pg = &doc.page;
    let sect = format!(
        "<w:sectPr><w:pgSz w:w=\"{}\" w:h=\"{}\"/><w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/></w:sectPr>",
        pg.width, pg.height, pg.top, pg.right, pg.bottom, pg.left
    );
    let document = format!(
        "{XML_DECL}<w:document xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\" xmlns:wp=\"{NS_WP}\"><w:body>{body}{sect}</w:body></w:document>"
    );

    let mut nums = String::new();
    for i in 0..numbered_lists {
        nums.push_str(&format!(
            "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"1\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"1\"/></w:lvlOverride></w:num>",
            i + 2
        ));
    }
    let numbering = format!(
        "{XML_DECL}<w:numbering xmlns:w=\"{NS_W}\">\
<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"hybridMultilevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"•\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl></w:abstractNum>\
<w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"hybridMultilevel\"/><w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%1.\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr></w:lvl></w:abstractNum>\
<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>{nums}</w:numbering>"
    );

    let heading = |n: u8, half_pts: u32| {
        format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{n}\"><w:name w:val=\"heading {n}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/>\
<w:pPr><w:keepNext/><w:outlineLvl w:val=\"{}\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"{half_pts}\"/></w:rPr></w:style>",
            n - 1
        )
    };
    // Body text defaults. Without them Word falls back to 10 pt Times New Roman.
    let font = esc_attr(doc.defaults.font.as_deref().unwrap_or(DEFAULT_FONT));
    let size = doc.defaults.size.unwrap_or(24);
    let doc_defaults = format!(
        "<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:cs=\"{font}\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/></w:rPr></w:rPrDefault></w:docDefaults>"
    );
    let styles = format!(
        "{XML_DECL}<w:styles xmlns:w=\"{NS_W}\">{doc_defaults}\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>{}{}{}\
<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\"><w:name w:val=\"List Paragraph\"/><w:basedOn w:val=\"Normal\"/><w:qFormat/><w:pPr><w:ind w:left=\"720\"/></w:pPr></w:style></w:styles>",
        heading(1, 40),
        heading(2, 32),
        heading(3, 28)
    );

    let content_types = format!(
        "{XML_DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/>\
<Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
<Default Extension=\"gif\" ContentType=\"image/gif\"/>\
<Default Extension=\"webp\" ContentType=\"image/webp\"/>\
<Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/></Types>"
    );
    let rels = format!(
        "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>"
    );
    let image_rels: String = media_ids
        .values()
        .map(|(rid, file)| {
            format!("<Relationship Id=\"{rid}\" Type=\"{NS_R}/image\" Target=\"media/{file}\"/>")
        })
        .collect();
    let doc_rels = format!(
        "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>{image_rels}</Relationships>"
    );

    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in [
        ("[Content_Types].xml", &content_types),
        ("_rels/.rels", &rels),
        ("word/_rels/document.xml.rels", &doc_rels),
        ("word/document.xml", &document),
        ("word/styles.xml", &styles),
        ("word/numbering.xml", &numbering),
    ] {
        z.start_file(name, opts)?;
        z.write_all(data.as_bytes())?;
    }
    for (name, data) in &media {
        z.start_file(format!("word/media/{name}"), opts)?;
        z.write_all(data)?;
    }
    Ok(z.finish()?.into_inner())
}

fn ext_for(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpeg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        _ => None,
    }
}

/// An inline picture run; sizes are CSS px (9525 EMU each).
fn drawing_xml(rid: &str, n: usize, w: u32, h: u32) -> String {
    let (cx, cy) = (w as u64 * 9525, h as u64 * 9525);
    format!(
        "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
<wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"Picture {n}\"/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
<a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"Picture {n}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>\
</wp:inline></w:drawing></w:r>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quill_core::Pos;

    #[test]
    fn round_trip_formats_kinds_and_special_chars() {
        let mut d = Document::from_paragraphs(vec![
            Paragraph::plain("Title & <more>").with_kind(Kind::Heading(2)),
            Paragraph::plain("Hello\tworld\nnext line"),
            Paragraph::plain("one").with_kind(Kind::Bullet),
            Paragraph::plain("two").with_kind(Kind::Numbered),
            Paragraph::plain("three").with_kind(Kind::Numbered),
            Paragraph::plain("break"),
            Paragraph::plain("again").with_kind(Kind::Numbered),
            Paragraph::plain(""),
        ]);
        d.set_format(1, 0, 5, |f| f.bold = true);
        d.set_format(1, 3, 8, |f| f.underline = true);
        d.set_format(0, 0, 2, |f| f.italic = true);
        let back = read(&write(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn separate_numbered_lists_get_separate_instances() {
        let d = Document::from_paragraphs(vec![
            Paragraph::plain("a").with_kind(Kind::Numbered),
            Paragraph::plain("x"),
            Paragraph::plain("b").with_kind(Kind::Numbered),
        ]);
        let bytes = write(&d).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let n = read_part(&mut z, "word/numbering.xml").unwrap().unwrap();
        assert!(n.contains("w:numId=\"2\"") && n.contains("w:numId=\"3\""));
    }

    #[test]
    fn rejects_non_docx() {
        assert!(read(b"nope").is_err());
    }

    #[test]
    fn missing_document_part_is_reported() {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        z.start_file("hello.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        let bytes = z.finish().unwrap().into_inner();
        assert!(matches!(read(&bytes), Err(DocxError::MissingDocument)));
    }

    #[test]
    fn toggle_off_values_are_respected() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:rPr><w:b w:val="0"/><w:u w:val="none"/></w:rPr><w:t>a</w:t></w:r></w:p></w:body></w:document>"#;
        let d =
            parse_document_xml(xml, &HashMap::new(), &Media::new(), &Styles::default()).unwrap();
        assert_eq!(d.paragraphs[0].runs[0].format, Format::default());
    }

    #[test]
    fn word_style_ids_map_to_kinds() {
        assert_eq!(kind_from_style("Heading1"), Some(Kind::Heading(1)));
        assert_eq!(kind_from_style("heading 5"), Some(Kind::Heading(3)));
        assert_eq!(kind_from_style("ListBullet"), Some(Kind::Bullet));
        assert_eq!(kind_from_style("BodyText"), None);
    }

    #[test]
    fn empty_paragraphs_survive() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:p/><w:p><w:r><w:t>a</w:t></w:r></w:p></w:body></w:document>"#;
        let d =
            parse_document_xml(xml, &HashMap::new(), &Media::new(), &Styles::default()).unwrap();
        assert_eq!(d.paragraphs.len(), 2);
    }

    #[test]
    fn tables_round_trip() {
        let cell = |t: u32, r: u16, c: u16, text: &str| Paragraph {
            cell: Some(Cell {
                table: t,
                row: r,
                col: c,
            }),
            ..Paragraph::plain(text)
        };
        let mut second = cell(0, 1, 1, "d & <e>").with_kind(Kind::Numbered);
        second.cell = Some(Cell {
            table: 0,
            row: 1,
            col: 1,
        });
        let d = Document::from_paragraphs(vec![
            Paragraph::plain("intro"),
            cell(0, 0, 0, "a"),
            cell(0, 0, 0, "a2"),
            cell(0, 0, 1, "b").with_kind(Kind::Heading(2)),
            cell(0, 1, 0, "c"),
            second,
            Paragraph::plain("between"),
            cell(1, 0, 0, "solo"),
            Paragraph::plain("outro"),
        ]);
        assert_eq!(d.table_dims(0), Some((2, 2)));
        let bytes = write(&d).unwrap();
        let back = read(&bytes).unwrap();
        assert_eq!(back, d);
        let mut z = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let xml = read_part(&mut z, "word/document.xml").unwrap().unwrap();
        assert_eq!(xml.matches("<w:tbl>").count(), 2);
        assert_eq!(xml.matches("<w:tr>").count(), 3);
        assert_eq!(xml.matches("<w:tc>").count(), 5);
    }

    #[test]
    fn table_at_end_gets_a_trailing_paragraph() {
        let d = Document::from_paragraphs(vec![Paragraph {
            cell: Some(Cell {
                table: 0,
                row: 0,
                col: 0,
            }),
            ..Paragraph::plain("x")
        }]);
        let back = read(&write(&d).unwrap()).unwrap();
        assert_eq!(back.paragraphs.len(), 2);
        assert_eq!(back, d);
    }

    #[test]
    fn nested_table_paragraphs_fold_into_outer_cell() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:tbl><w:tr><w:tc><w:p><w:r><w:t>outer</w:t></w:r></w:p>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:tc><w:tc><w:p><w:r><w:t>next</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;
        let d =
            parse_document_xml(xml, &HashMap::new(), &Media::new(), &Styles::default()).unwrap();
        let cells: Vec<_> = d
            .paragraphs
            .iter()
            .filter_map(|p| p.cell.map(|c| (c.col, p.text())))
            .collect();
        assert_eq!(
            cells,
            vec![(0, "outer".into()), (0, "inner".into()), (1, "next".into())]
        );
    }

    fn png_bytes(tag: u8) -> Vec<u8> {
        vec![0x89, b'P', b'N', b'G', tag]
    }

    #[test]
    fn images_round_trip_with_media_and_sizes() {
        let mut d =
            Document::from_paragraphs(vec![Paragraph::plain("before after"), Paragraph::plain("")]);
        d.insert_image(
            Pos { para: 0, offset: 6 },
            Image::new("image/png", png_bytes(1), 120, 60),
        );
        d.insert_image(
            Pos { para: 1, offset: 0 },
            Image::new("image/jpeg", vec![0xFF, 0xD8, 9], 33, 44),
        );
        let bytes = write(&d).unwrap();
        let back = read(&bytes).unwrap();
        assert_eq!(back, d);
        assert_eq!(back.images.len(), 2);

        let mut z = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let mut got = Vec::new();
        z.by_name("word/media/image1.png")
            .unwrap()
            .read_to_end(&mut got)
            .unwrap();
        assert_eq!(got, png_bytes(1));
        assert!(z.by_name("word/media/image2.jpeg").is_ok());
        let xml = read_part(&mut z, "word/document.xml").unwrap().unwrap();
        assert!(
            xml.contains("cx=\"1143000\"") && xml.contains("cy=\"571500\""),
            "120×60 px in EMU"
        );
    }

    #[test]
    fn unsupported_image_types_are_dropped_not_corrupting() {
        let mut d = Document::from_paragraphs(vec![Paragraph::plain("ab")]);
        d.insert_image(
            Pos { para: 0, offset: 1 },
            Image::new("image/svg+xml", b"<svg/>".to_vec(), 10, 10),
        );
        let back = read(&write(&d).unwrap()).unwrap();
        assert_eq!(back.plain_text(), "ab");
        assert!(back.images.is_empty());
    }

    #[test]
    fn anchored_images_and_missing_media_are_handled() {
        let xml = r#"<w:document xmlns:w="w" xmlns:wp="wp" xmlns:a="a" xmlns:r="r"><w:body><w:p>
            <w:r><w:t>x</w:t></w:r>
            <w:r><w:drawing><wp:anchor><wp:extent cx="952500" cy="476250"/><wp:effectExtent l="0" t="0" r="0" b="0"/>
              <a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId9"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>
            <w:r><w:drawing><wp:inline><wp:extent cx="1" cy="1"/><a:blip r:embed="rId404"/></wp:inline></w:drawing></w:r>
            </w:p></w:body></w:document>"#;
        let mut media = Media::new();
        media.insert("rId9".into(), ("image/png".into(), png_bytes(5)));
        let d = parse_document_xml(xml, &HashMap::new(), &media, &Styles::default()).unwrap();
        assert_eq!(d.images.len(), 1, "unresolvable relationship is skipped");
        assert_eq!((d.images[0].width, d.images[0].height), (100, 50));
        assert_eq!(d.paragraphs[0].len(), 2);
    }

    #[test]
    fn character_and_paragraph_formatting_round_trip() {
        let mut d = Document::from_paragraphs(vec![
            Paragraph::plain("Big red Courier & <more>"),
            Paragraph::plain("centered"),
            Paragraph::plain("right").with_kind(Kind::Bullet),
            Paragraph::plain("justified"),
            Paragraph::plain("plain"),
        ]);
        d.set_format(0, 0, 3, |f| f.size = Some(48));
        d.set_format(0, 4, 7, |f| f.color = Some(0xC0392B));
        d.set_format(0, 8, 15, |f| {
            f.font = Some("Courier New".into());
            f.size = Some(21); // 10.5pt
            f.bold = true;
        });
        d.paragraphs[1].align = Align::Center;
        d.paragraphs[2].align = Align::Right;
        d.paragraphs[3].align = Align::Justify;
        let bytes = write(&d).unwrap();
        assert_eq!(read(&bytes).unwrap(), d);

        let mut z = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let xml = read_part(&mut z, "word/document.xml").unwrap().unwrap();
        assert!(xml.contains("<w:sz w:val=\"48\"/>") && xml.contains("<w:sz w:val=\"21\"/>"));
        assert!(xml.contains("<w:color w:val=\"C0392B\"/>"));
        assert!(xml.contains("w:ascii=\"Courier New\""));
        assert!(xml.contains("<w:jc w:val=\"both\"/>"));
        // Schema order: pStyle, numPr, then jc.
        let ppr = xml.split("<w:pPr>").find(|p| p.contains("numPr")).unwrap();
        assert!(ppr.find("numPr").unwrap() < ppr.find("<w:jc").unwrap());
    }

    #[test]
    fn word_style_values_are_understood() {
        let xml = r#"<w:document xmlns:w="w"><w:body>
          <w:p><w:pPr><w:jc w:val="end"/><w:rPr><w:sz w:val="40"/></w:rPr></w:pPr>
            <w:r><w:rPr><w:rFonts w:hAnsi="Calibri"/><w:color w:val="auto"/><w:sz w:val="22"/></w:rPr><w:t>x</w:t></w:r></w:p>
          <w:tbl><w:tblPr><w:jc w:val="center"/></w:tblPr><w:tr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
        </w:body></w:document>"#;
        let d =
            parse_document_xml(xml, &HashMap::new(), &Media::new(), &Styles::default()).unwrap();
        let p = &d.paragraphs[0];
        assert_eq!(p.align, Align::Right);
        let f = &p.runs[0].format;
        assert_eq!(
            (f.size, f.font.as_deref(), f.color),
            (Some(22), Some("Calibri"), None)
        );
        assert_eq!(
            d.paragraphs[1].align,
            Align::Left,
            "table jc must not leak into the cell paragraph"
        );
    }

    /// Build a .docx from raw parts.
    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in parts {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(data.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    const THEME: &str = r#"<a:theme xmlns:a="a"><a:themeElements><a:fontScheme>
        <a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/></a:majorFont>
        <a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/></a:minorFont>
        </a:fontScheme></a:themeElements></a:theme>"#;

    // What Word typically writes: theme fonts, 11pt, 8pt after, 1.08 lines.
    const WORD_STYLES: &str = r#"<w:styles xmlns:w="w">
      <w:docDefaults>
        <w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi"/><w:sz w:val="22"/></w:rPr></w:rPrDefault>
        <w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault>
      </w:docDefaults>
      <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
      <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/>
        <w:pPr><w:keepNext/><w:spacing w:before="240" w:after="0"/><w:rPr><w:sz w:val="99"/></w:rPr></w:pPr>
        <w:rPr><w:rFonts w:asciiTheme="majorHAnsi"/><w:color w:val="2F5496"/><w:sz w:val="32"/></w:rPr></w:style>
      <w:style w:type="paragraph" w:styleId="Quote"><w:basedOn w:val="Normal"/>
        <w:pPr><w:ind w:left="864" w:right="864"/><w:jc w:val="center"/></w:pPr><w:rPr><w:i/></w:rPr></w:style>
      <w:style w:type="paragraph" w:styleId="Loop1"><w:basedOn w:val="Loop2"/><w:rPr><w:sz w:val="50"/></w:rPr></w:style>
      <w:style w:type="paragraph" w:styleId="Loop2"><w:basedOn w:val="Loop1"/><w:rPr><w:b/></w:rPr></w:style>
      <w:style w:type="character" w:styleId="Strong"><w:rPr><w:b/><w:color w:val="C00000"/></w:rPr></w:style>
      <w:style w:type="table" w:styleId="TableGrid"><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr>
        <w:tblStylePr w:type="firstRow"><w:pPr><w:spacing w:after="999"/></w:pPr><w:rPr><w:b/></w:rPr></w:tblStylePr></w:style>
    </w:styles>"#;

    fn word_like(body: &str) -> Document {
        let doc = format!(
            r#"<w:document xmlns:w="w"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/>
               <w:pgMar w:top="1134" w:right="850" w:bottom="1701" w:left="1417" w:header="708" w:footer="708"/></w:sectPr></w:body></w:document>"#
        );
        let bytes = package(&[
            ("word/document.xml", &doc),
            ("word/styles.xml", WORD_STYLES),
            ("word/theme/theme1.xml", THEME),
        ]);
        read(&bytes).unwrap()
    }

    #[test]
    fn document_defaults_page_and_theme_fonts_are_imported() {
        let d = word_like(r#"<w:p><w:r><w:t>plain</w:t></w:r></w:p>"#);
        assert_eq!(
            d.defaults.font.as_deref(),
            Some("Calibri"),
            "minor theme font"
        );
        assert_eq!(d.defaults.size, Some(22));
        let p = &d.paragraphs[0];
        assert_eq!(
            (p.space_after, p.space_before, p.line_spacing),
            (Some(160), None, Some(259))
        );
        // Plain text stays plain: defaults are not repeated on every run.
        assert_eq!(p.runs[0].format, Format::default());
        assert_eq!(
            d.page,
            Page {
                width: 11906,
                height: 16838,
                top: 1134,
                right: 850,
                bottom: 1701,
                left: 1417
            }
        );
    }

    #[test]
    fn paragraph_styles_inherit_and_direct_formatting_wins() {
        let d = word_like(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>
               <w:p><w:pPr><w:pStyle w:val="Quote"/><w:jc w:val="right"/></w:pPr><w:r><w:t>said</w:t></w:r></w:p>
               <w:p><w:r><w:rPr><w:sz w:val="40"/><w:i w:val="0"/></w:rPr><w:t>big</w:t></w:r></w:p>"#,
        );
        let h = &d.paragraphs[0];
        assert_eq!(h.kind, Kind::Heading(1));
        // basedOn Normal → docDefaults line spacing; style's own before/after win.
        assert_eq!(
            (h.space_before, h.space_after, h.line_spacing),
            (Some(240), Some(0), Some(259))
        );
        let f = &h.runs[0].format;
        assert_eq!(f.color, Some(0x2F5496));
        assert_eq!(f.font.as_deref(), Some("Calibri Light"), "major theme font");
        assert_eq!(
            f.size,
            Some(32),
            "16pt differs from Quill's 20pt heading default"
        );
        assert!(!f.bold, "Quill headings are bold by kind, not by run");

        let q = &d.paragraphs[1];
        assert_eq!(q.kind, Kind::Normal);
        assert_eq!(q.indent_left, Some(864));
        assert_eq!(
            q.align,
            Align::Right,
            "direct jc overrides the style's centre"
        );
        assert!(
            q.runs[0].format.italic,
            "style's run properties apply to its text"
        );

        let big = &d.paragraphs[2].runs[0].format;
        assert_eq!(big.size, Some(40));
    }

    #[test]
    fn character_styles_and_toggles_layer_correctly() {
        let d = word_like(
            r#"<w:p><w:pPr><w:pStyle w:val="Quote"/></w:pPr>
                 <w:r><w:rPr><w:rStyle w:val="Strong"/></w:rPr><w:t>a</w:t></w:r>
                 <w:r><w:rPr><w:rStyle w:val="Strong"/><w:b w:val="0"/><w:i w:val="false"/></w:rPr><w:t>b</w:t></w:r></w:p>"#,
        );
        let runs = &d.paragraphs[0].runs;
        assert_eq!(runs.len(), 2);
        assert!(
            runs[0].format.bold && runs[0].format.italic,
            "char style + paragraph style"
        );
        assert_eq!(runs[0].format.color, Some(0xC00000));
        assert!(
            !runs[1].format.bold && !runs[1].format.italic,
            "direct values beat both styles"
        );
        assert_eq!(runs[1].format.color, Some(0xC00000));
    }

    #[test]
    fn table_styles_set_cell_spacing_but_not_conditional_formatting() {
        let d = word_like(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr><w:tr>
                 <w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
               <w:p><w:r><w:t>after</w:t></w:r></w:p>"#,
        );
        let c = &d.paragraphs[0];
        assert!(c.cell.is_some());
        assert_eq!((c.space_after, c.line_spacing), (Some(0), Some(240)));
        assert!(
            !c.runs[0].format.bold,
            "firstRow conditional formatting is ignored"
        );
        assert_eq!(
            d.paragraphs[1].space_after,
            Some(160),
            "text outside the table is unaffected"
        );
    }

    #[test]
    fn style_cycles_and_unknown_styles_do_not_hang_or_panic() {
        let d = word_like(
            r#"<w:p><w:pPr><w:pStyle w:val="Loop1"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>
               <w:p><w:pPr><w:pStyle w:val="NoSuchStyle"/></w:pPr><w:r><w:rPr><w:rStyle w:val="Nope"/></w:rPr><w:t>y</w:t></w:r></w:p>"#,
        );
        assert_eq!(d.paragraphs.len(), 2);
        assert_eq!(d.paragraphs[0].runs[0].format.size, Some(50));
    }

    #[test]
    fn missing_styles_theme_and_section_fall_back_to_defaults() {
        let bytes = package(&[(
            "word/document.xml",
            r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>"#,
        )]);
        let d = read(&bytes).unwrap();
        assert_eq!(d.page, Page::default());
        assert_eq!(d.defaults, Defaults::default());
        assert_eq!(d.paragraphs[0].space_after, None);
    }

    #[test]
    fn spacing_indent_page_and_defaults_round_trip() {
        let mut d = Document::from_paragraphs(vec![
            Paragraph::plain("spaced"),
            Paragraph::plain("hanging"),
            Paragraph::plain("first line"),
            Paragraph::plain("item").with_kind(Kind::Bullet),
        ]);
        d.paragraphs[0].space_before = Some(120);
        d.paragraphs[0].space_after = Some(0);
        d.paragraphs[0].line_spacing = Some(360);
        d.paragraphs[1].indent_left = Some(720);
        d.paragraphs[1].indent_first = Some(-360);
        d.paragraphs[2].indent_first = Some(240);
        d.paragraphs[3].indent_left = Some(999); // ignored for lists
        d.page = Page::A4;
        d.page.left = 1700;
        d.defaults = Defaults {
            font: Some("Calibri".into()),
            size: Some(22),
        };
        let bytes = write(&d).unwrap();
        let mut back = read(&bytes).unwrap();
        d.paragraphs[3].indent_left = None;
        assert_eq!(back, d);
        // Quill's own default font is not written back as an explicit default.
        back.defaults = Defaults::default();
        assert_eq!(
            read(&write(&back).unwrap()).unwrap().defaults,
            Defaults::default()
        );

        let mut z = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let xml = read_part(&mut z, "word/document.xml").unwrap().unwrap();
        assert!(xml
            .contains(r#"<w:spacing w:before="120" w:after="0" w:line="360" w:lineRule="auto"/>"#));
        assert!(xml.contains(r#"<w:ind w:left="720" w:hanging="360"/>"#));
        assert!(xml.contains(r#"<w:ind w:firstLine="240"/>"#));
        assert!(
            xml.contains(r#"<w:pgSz w:w="11906" w:h="16838"/>"#)
                && xml.contains(r#"w:left="1700""#)
        );
        assert!(
            xml.find("<w:sectPr>").unwrap() > xml.rfind("</w:p>").unwrap(),
            "sectPr closes the body"
        );
        let styles = read_part(&mut z, "word/styles.xml").unwrap().unwrap();
        assert!(
            styles.contains(r#"<w:rFonts w:ascii="Calibri""#)
                && styles.contains(r#"<w:sz w:val="22"/>"#)
        );
    }
}
