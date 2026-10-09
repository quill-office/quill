//! `.docx` (OOXML WordprocessingML) reader/writer.
//!
//! Supported: paragraphs, runs with bold/italic/underline, tabs and line
//! breaks, Heading 1–3, bulleted and numbered lists (single level). Anything
//! else (tables, images, other styles, …) is dropped on import.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use quill_core::{Document, Format, Kind, Paragraph, Run};

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
    parse_document_xml(&xml, &numbering)
}

fn attr(e: &BytesStart, local: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == local)
        .map(|a| a.value.to_string())
}

/// `<w:b/>`-style toggles are on unless `w:val` says otherwise.
fn toggle_on(e: &BytesStart) -> bool {
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

fn parse_document_xml(xml: &str, numbering: &HashMap<u32, bool>) -> Result<Document, DocxError> {
    let mut r = Reader::from_str(xml);
    let mut paras: Vec<Paragraph> = Vec::new();
    let mut cur: Option<Paragraph> = None;
    let mut style_kind: Option<Kind> = None;
    let mut num_kind: Option<Kind> = None;
    let mut fmt = Format::default();
    let mut in_text = false;
    loop {
        match r.read_event()? {
            Event::Start(e) if e.local_name().as_ref() == "p" => {
                cur = Some(Paragraph::default());
                style_kind = None;
                num_kind = None;
            }
            Event::Empty(e) if e.local_name().as_ref() == "p" => paras.push(Paragraph::default()),
            Event::Start(e) => match e.local_name().as_ref() {
                "r" => fmt = Format::default(),
                "b" => fmt.bold = toggle_on(&e),
                "i" => fmt.italic = toggle_on(&e),
                "u" => fmt.underline = toggle_on(&e),
                "pStyle" => style_kind = attr(&e, "val").and_then(|v| kind_from_style(&v)),
                "numId" => num_kind = numbering_kind(&e, numbering),
                "t" => in_text = true,
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "b" => fmt.bold = toggle_on(&e),
                "i" => fmt.italic = toggle_on(&e),
                "u" => fmt.underline = toggle_on(&e),
                "pStyle" => style_kind = attr(&e, "val").and_then(|v| kind_from_style(&v)),
                "numId" => num_kind = numbering_kind(&e, numbering),
                "tab" => push(&mut cur, "\t", &fmt),
                "br" => push(&mut cur, "\n", &fmt),
                _ => {}
            },
            Event::Text(t) if in_text => push(
                &mut cur,
                &t.xml_content(quick_xml::XmlVersion::Implicit1_0),
                &fmt,
            ),
            Event::GeneralRef(g) if in_text => {
                let s = match g.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => "",
                };
                push(&mut cur, s, &fmt);
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "p" => {
                    if let Some(mut p) = cur.take() {
                        // A list numbering wins over a plain style.
                        p.kind = num_kind.or(style_kind).unwrap_or(Kind::Normal);
                        paras.push(p);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(Document::from_paragraphs(paras))
}

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

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn run_xml(text: &str, f: &Format) -> String {
    let mut props = String::new();
    if f.bold {
        props.push_str("<w:b/>");
    }
    if f.italic {
        props.push_str("<w:i/>");
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
const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>";

pub fn write(doc: &Document) -> Result<Vec<u8>, DocxError> {
    // Each contiguous run of numbered paragraphs gets its own list instance so
    // numbering restarts at 1, as it does in Quill.
    let mut body = String::new();
    let mut numbered_lists = 0u32;
    let mut prev_numbered = false;
    for p in &doc.paragraphs {
        let ppr = match p.kind {
            Kind::Normal => String::new(),
            Kind::Heading(n) => format!("<w:pPr><w:pStyle w:val=\"Heading{n}\"/></w:pPr>"),
            Kind::Bullet => "<w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr>".to_string(),
            Kind::Numbered => {
                if !prev_numbered {
                    numbered_lists += 1;
                }
                format!("<w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{}\"/></w:numPr></w:pPr>", numbered_lists + 1)
            }
        };
        prev_numbered = p.kind == Kind::Numbered;
        body.push_str("<w:p>");
        body.push_str(&ppr);
        for r in &p.runs {
            body.push_str(&run_xml(&r.text, &r.format));
        }
        body.push_str("</w:p>");
    }
    let document =
        format!("{XML_DECL}<w:document xmlns:w=\"{NS_W}\"><w:body>{body}</w:body></w:document>");

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
<w:pPr><w:keepNext/><w:spacing w:before=\"240\" w:after=\"80\"/><w:outlineLvl w:val=\"{}\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"{half_pts}\"/></w:rPr></w:style>",
            n - 1
        )
    };
    let styles = format!(
        "{XML_DECL}<w:styles xmlns:w=\"{NS_W}\">\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>{}{}{}\
<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\"><w:name w:val=\"List Paragraph\"/><w:basedOn w:val=\"Normal\"/><w:qFormat/><w:pPr><w:ind w:left=\"720\"/></w:pPr></w:style></w:styles>",
        heading(1, 56),
        heading(2, 44),
        heading(3, 38)
    );

    let content_types = format!(
        "{XML_DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/></Types>"
    );
    let rels = format!(
        "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>"
    );
    let doc_rels = format!(
        "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/></Relationships>"
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
    Ok(z.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let d = parse_document_xml(xml, &HashMap::new()).unwrap();
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
        let d = parse_document_xml(xml, &HashMap::new()).unwrap();
        assert_eq!(d.paragraphs.len(), 2);
    }
}
