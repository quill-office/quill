//! Searching a story's text.

use crate::model::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub match_case: bool,
    /// Matches must not touch other word characters.
    pub whole_word: bool,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// One char in, one char out, so offsets stay valid after folding case.
fn fold(c: char, match_case: bool) -> char {
    if match_case {
        c
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

impl Document {
    /// All non-overlapping matches of `query` in the story being edited, in
    /// document order. Matches never span paragraphs.
    pub fn find_all(&self, query: &str, opts: FindOptions) -> Vec<(Pos, Pos)> {
        let q: Vec<char> = query.chars().map(|c| fold(c, opts.match_case)).collect();
        if q.is_empty() {
            return vec![];
        }
        let check_start = opts.whole_word && is_word(q[0]);
        let check_end = opts.whole_word && is_word(q[q.len() - 1]);
        let mut out = Vec::new();
        for (pi, p) in self.paragraphs.iter().enumerate() {
            let text: Vec<char> = p.text().chars().map(|c| fold(c, opts.match_case)).collect();
            if text.len() < q.len() {
                continue;
            }
            let mut i = 0;
            while i + q.len() <= text.len() {
                let end = i + q.len();
                let hit = text[i..end] == q[..]
                    && !(check_start && i > 0 && is_word(text[i - 1]))
                    && !(check_end && end < text.len() && is_word(text[end]));
                if hit {
                    out.push((
                        Pos {
                            para: pi,
                            offset: i,
                        },
                        Pos {
                            para: pi,
                            offset: end,
                        },
                    ));
                    i = end;
                } else {
                    i += 1;
                }
            }
        }
        out
    }
}
