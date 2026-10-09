//! `word/styles.xml` and theme import.
//!
//! Word resolves a run's look from several layers; Quill has no style
//! objects, so import flattens them into explicit run/paragraph properties.
//! Layers, lowest to highest priority: document defaults → table style →
//! paragraph style chain (`basedOn`) → character style chain → direct
//! formatting. Conditional table-style formatting (banded rows, first column,
//! …) is not applied.

use std::collections::HashMap;
use std::sync::Arc;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use quill_core::{Align, Defaults};

use crate::{attr, toggle_on, DocxError};

/// Run properties where `None` means "not specified at this layer".
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RProps {
    pub font: Option<Arc<str>>,
    /// Half-points.
    pub size: Option<u16>,
    pub color: Option<u32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
}

impl RProps {
    /// Let `o` override whatever it specifies.
    pub fn overlay(&mut self, o: &RProps) {
        if o.font.is_some() {
            self.font = o.font.clone();
        }
        self.size = o.size.or(self.size);
        self.color = o.color.or(self.color);
        self.bold = o.bold.or(self.bold);
        self.italic = o.italic.or(self.italic);
        self.underline = o.underline.or(self.underline);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PProps {
    pub align: Option<Align>,
    /// Twips.
    pub before: Option<u16>,
    pub after: Option<u16>,
    /// 240ths of a line (only for "auto" line rules).
    pub line: Option<u16>,
    pub left: Option<u16>,
    pub first: Option<i16>,
}

impl PProps {
    pub fn overlay(&mut self, o: &PProps) {
        self.align = o.align.or(self.align);
        self.before = o.before.or(self.before);
        self.after = o.after.or(self.after);
        self.line = o.line.or(self.line);
        self.left = o.left.or(self.left);
        self.first = o.first.or(self.first);
    }
}

/// Theme font names (`+mj-lt` / `+mn-lt`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Theme {
    pub major: Option<Arc<str>>,
    pub minor: Option<Arc<str>>,
}

pub(crate) fn parse_theme(xml: &str) -> Result<Theme, DocxError> {
    let mut r = Reader::from_str(xml);
    let mut theme = Theme::default();
    let mut section = "";
    loop {
        match r.read_event()? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "majorFont" => section = "major",
                "minorFont" => section = "minor",
                "latin" => {
                    let face = attr(&e, "typeface")
                        .filter(|f| !f.is_empty())
                        .map(Arc::<str>::from);
                    match section {
                        "major" => theme.major = theme.major.take().or(face),
                        "minor" => theme.minor = theme.minor.take().or(face),
                        _ => {}
                    }
                }
                _ => {}
            },
            Event::End(e) if matches!(e.local_name().as_ref(), "majorFont" | "minorFont") => {
                section = ""
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(theme)
}

/// Apply one run-property element (`b`, `i`, `u`, `sz`, `color`, `rFonts`).
/// Returns whether the element was a run property.
pub(crate) fn apply_run_prop(e: &BytesStart, r: &mut RProps, theme: &Theme) -> bool {
    match e.local_name().as_ref() {
        "rFonts" => {
            let themed = attr(e, "asciiTheme")
                .or_else(|| attr(e, "hAnsiTheme"))
                .and_then(|t| {
                    if t.starts_with("major") {
                        theme.major.clone()
                    } else if t.starts_with("minor") {
                        theme.minor.clone()
                    } else {
                        None
                    }
                });
            let named = attr(e, "ascii")
                .or_else(|| attr(e, "hAnsi"))
                .filter(|n| !n.is_empty())
                .map(Arc::<str>::from);
            if let Some(f) = themed.or(named) {
                r.font = Some(f);
            }
        }
        "sz" => r.size = attr(e, "val").and_then(|v| v.parse().ok()).or(r.size),
        "color" => {
            // "auto" (or anything that isn't #RRGGBB) leaves the colour unspecified.
            if let Some(c) = attr(e, "val")
                .filter(|v| v.len() == 6)
                .and_then(|v| u32::from_str_radix(&v, 16).ok())
            {
                r.color = Some(c);
            }
        }
        "b" => r.bold = Some(toggle_on(e)),
        "i" => r.italic = Some(toggle_on(e)),
        "u" => r.underline = Some(toggle_on(e)),
        _ => return false,
    }
    true
}

fn twips(v: Option<String>) -> Option<i32> {
    v.and_then(|v| v.parse::<i32>().ok())
}

/// Apply one paragraph-property element (`jc`, `spacing`, `ind`).
pub(crate) fn apply_para_prop(e: &BytesStart, p: &mut PProps) -> bool {
    match e.local_name().as_ref() {
        "jc" => {
            p.align = match attr(e, "val").as_deref() {
                Some("center") => Some(Align::Center),
                Some("right" | "end") => Some(Align::Right),
                Some("both" | "distribute") => Some(Align::Justify),
                Some("left" | "start") => Some(Align::Left),
                _ => p.align,
            };
        }
        "spacing" => {
            let clamp = |v: i32| v.clamp(0, u16::MAX as i32) as u16;
            p.before = twips(attr(e, "before")).map(clamp).or(p.before);
            p.after = twips(attr(e, "after")).map(clamp).or(p.after);
            // Only proportional ("auto") line spacing maps onto Quill's model.
            if matches!(attr(e, "lineRule").as_deref(), None | Some("auto")) {
                p.line = twips(attr(e, "line")).map(clamp).or(p.line);
            }
        }
        "ind" => {
            let clamp = |v: i32| v.clamp(0, u16::MAX as i32) as u16;
            p.left = twips(attr(e, "left").or_else(|| attr(e, "start")))
                .map(clamp)
                .or(p.left);
            if let Some(h) = twips(attr(e, "hanging")) {
                p.first = Some(-(h.clamp(0, i16::MAX as i32) as i16));
            } else if let Some(f) = twips(attr(e, "firstLine")) {
                p.first = Some(f.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
            }
        }
        _ => return false,
    }
    true
}

#[derive(Clone, Debug, Default)]
struct Style {
    based_on: Option<String>,
    r: RProps,
    p: PProps,
}

#[derive(Debug, Default)]
pub(crate) struct Styles {
    doc_r: RProps,
    doc_p: PProps,
    styles: HashMap<String, Style>,
    default_para: Option<String>,
    pub theme: Theme,
}

impl Styles {
    pub fn parse(xml: &str, theme: Theme) -> Result<Styles, DocxError> {
        let mut r = Reader::from_str(xml);
        let mut out = Styles {
            theme,
            ..Default::default()
        };
        // Parser state.
        let (mut in_defaults, mut in_ppr, mut in_rpr, mut in_mark, mut in_cond) =
            (false, false, false, false, false);
        let mut cur: Option<(String, bool, bool, Style)> = None; // id, is_default, is_paragraph, style
        loop {
            let ev = r.read_event()?;
            match &ev {
                Event::Start(e) | Event::Empty(e) => {
                    let start = matches!(ev, Event::Start(_));
                    match e.local_name().as_ref() {
                        "docDefaults" => in_defaults = start,
                        "tblStylePr" => in_cond = start,
                        "pPr" => in_ppr = start,
                        "rPr" => {
                            if in_ppr {
                                in_mark = start; // paragraph-mark properties: not text formatting
                            } else {
                                in_rpr = start;
                            }
                        }
                        "style" if start => {
                            let id = attr(e, "styleId").unwrap_or_default();
                            let is_default = attr(e, "default")
                                .as_deref()
                                .is_some_and(|d| d == "1" || d == "true");
                            let is_para = attr(e, "type").as_deref() == Some("paragraph");
                            cur = Some((id, is_default, is_para, Style::default()));
                        }
                        "basedOn" => {
                            if let Some((_, _, _, s)) = cur.as_mut() {
                                s.based_on = attr(e, "val");
                            }
                        }
                        _ if in_cond || in_mark => {}
                        _ if in_rpr || in_ppr => {
                            let (r_target, p_target) = match cur.as_mut() {
                                Some((_, _, _, s)) => (&mut s.r, &mut s.p),
                                None if in_defaults => (&mut out.doc_r, &mut out.doc_p),
                                None => continue,
                            };
                            if in_rpr {
                                apply_run_prop(e, r_target, &out.theme);
                            } else {
                                apply_para_prop(e, p_target);
                            }
                        }
                        _ => {}
                    }
                }
                Event::End(e) => match e.local_name().as_ref() {
                    "docDefaults" => in_defaults = false,
                    "tblStylePr" => in_cond = false,
                    "pPr" => in_ppr = false,
                    "rPr" => {
                        in_rpr = false;
                        in_mark = false;
                    }
                    "style" => {
                        if let Some((id, is_default, is_para, style)) = cur.take() {
                            if is_default && is_para {
                                out.default_para = Some(id.clone());
                            }
                            out.styles.insert(id, style);
                        }
                    }
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(out)
    }

    /// Styles from `id` up through its `basedOn` ancestors, base first.
    fn chain(&self, id: Option<&str>) -> Vec<&Style> {
        let mut out = Vec::new();
        let mut next = id;
        while let Some(i) = next {
            // Guard against cycles in malformed files.
            let Some(s) = self.styles.get(i).filter(|_| out.len() < 32) else {
                break;
            };
            out.push(s);
            next = s.based_on.as_deref();
        }
        out.reverse();
        out
    }

    /// Effective properties of a paragraph before direct formatting.
    pub fn para_props(
        &self,
        table_style: Option<&str>,
        para_style: Option<&str>,
    ) -> (RProps, PProps) {
        let (mut r, mut p) = (self.doc_r.clone(), self.doc_p.clone());
        for s in self.chain(table_style) {
            r.overlay(&s.r);
            p.overlay(&s.p);
        }
        for s in self.chain(para_style.or(self.default_para.as_deref())) {
            r.overlay(&s.r);
            p.overlay(&s.p);
        }
        (r, p)
    }

    /// Run properties contributed by a character style.
    pub fn char_props(&self, id: &str) -> RProps {
        let mut r = RProps::default();
        for s in self.chain(Some(id)) {
            r.overlay(&s.r);
        }
        r
    }

    /// The body text defaults (document defaults plus the default paragraph style).
    pub fn defaults(&self) -> Defaults {
        let (r, _) = self.para_props(None, None);
        Defaults {
            font: r.font,
            size: r.size,
        }
    }
}
