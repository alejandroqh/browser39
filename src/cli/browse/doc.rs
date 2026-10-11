//! Line-oriented model of a rendered page for the interactive viewer:
//! markdown source lines, numbered link references, headings/sections,
//! soft-wrapped display rows and per-byte styling.

use std::fmt::Write as _;
use std::ops::Range;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::core::html_to_md::is_dangerous_char;
use crate::core::page::Link;
use crate::core::url::resolve_absolute;

/// One occurrence of a compact link reference `[text][n]` in the markdown.
#[derive(Debug)]
pub struct LinkRef {
    pub line: usize,
    /// Byte range of the whole `[text][n]` within the line.
    pub span: Range<usize>,
    /// Byte range of `text`.
    pub text: Range<usize>,
    /// Index into `Doc::links`.
    pub link: usize,
}

#[derive(Debug)]
pub struct Heading {
    pub line: usize,
    pub level: usize,
    /// Heading text with markdown syntax removed.
    pub text: String,
}

/// A display row: a slice of one source line after soft wrapping.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// How a byte of the source should be drawn.
#[derive(Debug, Clone, Copy, Default)]
pub struct Look {
    pub heading: u8,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
    /// Markdown syntax (markers, brackets, table pipes) drawn de-emphasized.
    pub dim: bool,
}

pub struct Doc {
    pub url: String,
    pub title: Option<String>,
    pub lines: Vec<String>,
    pub links: Vec<Link>,
    /// Link references in document order.
    pub refs: Vec<LinkRef>,
    pub headings: Vec<Heading>,
    /// Lines inside (or delimiting) a fenced code block.
    code: Vec<bool>,
}

impl Doc {
    pub fn new(url: String, title: Option<String>, markdown: &str, mut links: Vec<Link>) -> Self {
        // The converter shortens same-origin links to paths; the viewer follows
        // and copies them, so make them absolute again.
        for link in &mut links {
            link.href = resolve_absolute(&url, &link.href);
        }
        // Control and bidi-override characters would corrupt the terminal display
        let lines: Vec<String> = markdown
            .lines()
            .map(|l| {
                l.replace('\t', "    ")
                    .replace(|c: char| c.is_control() || is_dangerous_char(c), " ")
            })
            .collect();
        let mut refs = Vec::new();
        let mut headings = Vec::new();
        let mut code = Vec::with_capacity(lines.len());
        let mut in_fence = false;
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                code.push(true);
                continue;
            }
            code.push(in_fence);
            if in_fence {
                continue;
            }
            let first_ref = refs.len();
            find_refs(line, i, links.len(), &mut refs);
            if let Some((level, _)) = parse_heading(line) {
                let text = without_refs(line, &refs[first_ref..]);
                headings.push(Heading {
                    line: i,
                    level,
                    text: text[level..].replace(['*', '`'], "").trim().to_string(),
                });
            }
        }
        Self {
            url,
            title,
            lines,
            links,
            refs,
            headings,
            code,
        }
    }

    /// Link references on `line`, as a slice of `refs`.
    fn refs_on(&self, line: usize) -> &[LinkRef] {
        let start = self.refs.partition_point(|r| r.line < line);
        let end = self.refs.partition_point(|r| r.line <= line);
        &self.refs[start..end]
    }

    /// Lines of the section opened by heading `h`: up to the next heading of
    /// the same or a higher level.
    pub fn section_lines(&self, h: usize) -> Range<usize> {
        let Heading { line, level, .. } = self.headings[h];
        let end = self.headings[h + 1..]
            .iter()
            .find(|o| o.level <= level)
            .map_or(self.lines.len(), |o| o.line);
        line..end
    }

    /// The heading whose section contains `line` (nearest heading at or above it).
    pub fn heading_at(&self, line: usize) -> Option<usize> {
        self.headings.iter().rposition(|h| h.line <= line)
    }

    /// Find a section by heading text, case-insensitively: exact match first,
    /// then prefix, then substring.
    pub fn find_section(&self, name: &str) -> Option<usize> {
        let name = name.trim().to_lowercase();
        let texts: Vec<String> = self
            .headings
            .iter()
            .map(|h| h.text.to_lowercase())
            .collect();
        texts
            .iter()
            .position(|t| *t == name)
            .or_else(|| texts.iter().position(|t| t.starts_with(&name)))
            .or_else(|| texts.iter().position(|t| t.contains(&name)))
    }

    /// Find the heading a URL fragment points at (`#Lunar_flyby`, `#lunar-flyby`).
    pub fn find_anchor(&self, fragment: &str) -> Option<usize> {
        let want = anchor_key(fragment);
        if want.is_empty() {
            return None;
        }
        self.headings
            .iter()
            .position(|h| anchor_key(&h.text) == want)
    }

    /// Markdown for `lines`, followed by reference definitions for the links
    /// it uses so the text stays valid, self-contained markdown.
    pub fn markdown_for(&self, lines: Range<usize>) -> String {
        let mut out = self.lines[lines.clone()].join("\n");
        out.truncate(out.trim_end().len());
        let mut used: Vec<usize> = self
            .refs
            .iter()
            .filter(|r| lines.contains(&r.line))
            .map(|r| r.link)
            .collect();
        used.sort_unstable();
        used.dedup();
        if !used.is_empty() {
            out.push_str("\n\n");
            for n in used {
                let href = &self.links[n].href;
                if href.is_empty() || href.contains(char::is_whitespace) {
                    let _ = writeln!(out, "[{n}]: <{href}>");
                } else {
                    let _ = writeln!(out, "[{n}]: {href}");
                }
            }
            out.pop();
        }
        out
    }

    /// Soft-wrap every line to `width` display columns.
    pub fn wrap(&self, width: usize) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.lines.len());
        for (i, line) in self.lines.iter().enumerate() {
            wrap_line(line, width.max(1), |start, end| {
                rows.push(Row {
                    line: i,
                    start,
                    end,
                })
            });
        }
        rows
    }

    /// Per-byte styling for one line.
    pub fn line_looks(&self, line_no: usize) -> Vec<Look> {
        let line = &self.lines[line_no];
        let b = line.as_bytes();
        let n = b.len();
        if self.code[line_no] {
            return vec![
                Look {
                    code: true,
                    ..Look::default()
                };
                n
            ];
        }
        let trimmed = line.trim_start();
        let indent = n - trimmed.len();
        if is_rule(trimmed) {
            return vec![
                Look {
                    dim: true,
                    ..Look::default()
                };
                n
            ];
        }

        let mut base = Look::default();
        let mut marker = 0..0;
        if let Some((level, _)) = parse_heading(line) {
            base.heading = level as u8;
            marker = 0..level;
        } else if trimmed.starts_with('>') {
            base.italic = true;
            marker = indent..indent + 1;
        } else if let Some(len) = list_marker_len(trimmed) {
            marker = indent..indent + len;
        }
        let mut looks = vec![base; n];
        for l in &mut looks[marker.clone()] {
            l.dim = true;
        }
        if trimmed.starts_with('|') {
            let separator = trimmed
                .bytes()
                .all(|c| matches!(c, b'|' | b'-' | b':' | b' '));
            for (l, &c) in looks.iter_mut().zip(b) {
                if c == b'|' || separator {
                    l.dim = true;
                }
            }
        }
        for r in self.refs_on(line_no) {
            for l in &mut looks[r.span.clone()] {
                l.dim = true;
            }
            for l in &mut looks[r.text.clone()] {
                l.dim = false;
                l.link = true;
            }
        }

        // Inline code and emphasis
        let (mut bold, mut italic, mut code) = (false, false, false);
        let mut i = marker.end;
        while i < n {
            let c = b[i];
            if c == b'`' {
                code = !code;
                looks[i].code = true;
                looks[i].dim = true;
                i += 1;
                continue;
            }
            if code {
                looks[i].code = true;
                i += 1;
                continue;
            }
            if c == b'*' {
                let run = b[i..].iter().take_while(|&&c| c == b'*').count();
                let opens = b.get(i + run).is_some_and(|c| !c.is_ascii_whitespace());
                let closes = i > 0 && !b[i - 1].is_ascii_whitespace();
                let toggled = match run {
                    1 => toggle(&mut italic, opens, closes),
                    2 => toggle(&mut bold, opens, closes),
                    3 => {
                        let on = bold || italic;
                        let t = toggle(&mut bold, opens && !on, closes && on);
                        italic = bold;
                        t
                    }
                    _ => false,
                };
                for l in &mut looks[i..i + run] {
                    l.dim |= toggled;
                }
                i += run;
                continue;
            }
            looks[i].bold |= bold;
            looks[i].italic |= italic;
            i += 1;
        }
        looks
    }
}

/// Open or close an emphasis span. Returns whether the marker was syntax.
fn toggle(state: &mut bool, opens: bool, closes: bool) -> bool {
    if *state && closes {
        *state = false;
        true
    } else if !*state && opens {
        *state = true;
        true
    } else {
        false
    }
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&c| c == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = line[level..].strip_prefix(' ')?;
    Some((level, rest.trim()))
}

fn is_rule(trimmed: &str) -> bool {
    trimmed.len() >= 3 && trimmed.bytes().all(|c| c == b'-')
}

/// Length of a list marker (`- `, `* `, `12. `) at the start of `s`.
fn list_marker_len(s: &str) -> Option<usize> {
    if s.starts_with("- ") || s.starts_with("* ") {
        return Some(2);
    }
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0 && s[digits..].starts_with(". ")).then_some(digits + 2)
}

/// `line` with each link reference `[text][n]` replaced by its text.
fn without_refs(line: &str, refs: &[LinkRef]) -> String {
    let mut out = String::with_capacity(line.len());
    let mut pos = 0;
    for r in refs {
        out.push_str(&line[pos..r.span.start]);
        out.push_str(&line[r.text.clone()]);
        pos = r.span.end;
    }
    out.push_str(&line[pos..]);
    out
}

/// Comparable form of a heading or URL fragment: lowercase alphanumerics only.
fn anchor_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Record every `[text][n]` in `line` whose `n` is a valid link index.
fn find_refs(line: &str, line_no: usize, link_count: usize, out: &mut Vec<LinkRef>) {
    let b = line.as_bytes();
    let mut prev_end = 0;
    let mut from = 0;
    while let Some(off) = line[from..].find("][") {
        let close = from + off;
        from = close + 1;
        let num_start = close + 2;
        let digits = b[num_start..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        let num_end = num_start + digits;
        if digits == 0 || b.get(num_end) != Some(&b']') {
            continue;
        }
        let Ok(link) = line[num_start..num_end].parse::<usize>() else {
            continue;
        };
        if link >= link_count {
            continue;
        }
        let Some(open) = matching_open(&b[prev_end..close]).map(|o| o + prev_end) else {
            continue;
        };
        out.push(LinkRef {
            line: line_no,
            span: open..num_end + 1,
            text: open + 1..close,
            link,
        });
        prev_end = num_end + 1;
        from = prev_end;
    }
}

/// Index of the `[` that opens the bracketed text ending at the end of `b`.
fn matching_open(b: &[u8]) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &c) in b.iter().enumerate().rev() {
        match c {
            b']' => depth += 1,
            b'[' if depth == 0 => return Some(j),
            b'[' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Word-wrap one line, calling `emit(start, end)` with byte ranges per row.
fn wrap_line(line: &str, width: usize, mut emit: impl FnMut(usize, usize)) {
    let mut start = 0;
    let mut col = 0;
    let mut last_space: Option<usize> = None;
    for (i, ch) in line.char_indices() {
        let w = ch.width().unwrap_or(0);
        if col + w > width && i > start {
            let brk = last_space.filter(|&s| s > start).unwrap_or(i);
            emit(start, brk);
            start = brk;
            col = line[start..i].width();
            last_space = None;
        }
        col += w;
        if ch == ' ' {
            last_space = Some(i + 1);
        }
    }
    emit(start, line.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(i: usize, href: &str) -> Link {
        Link {
            i,
            text: String::new(),
            href: href.into(),
        }
    }

    fn doc(md: &str, links: usize) -> Doc {
        let links = (0..links)
            .map(|i| link(i, &format!("https://x.test/{i}")))
            .collect();
        Doc::new("https://x.test/".into(), None, md, links)
    }

    #[test]
    fn finds_link_refs() {
        let d = doc("a [Moon][0] and [[1]][1] end", 2);
        assert_eq!(d.refs.len(), 2);
        let line = &d.lines[0];
        assert_eq!(&line[d.refs[0].span.clone()], "[Moon][0]");
        assert_eq!(&line[d.refs[0].text.clone()], "Moon");
        assert_eq!(&line[d.refs[1].span.clone()], "[[1]][1]");
        assert_eq!(&line[d.refs[1].text.clone()], "[1]");
    }

    #[test]
    fn ignores_out_of_range_and_fenced_refs() {
        let d = doc("[a][5]\n```\n[b][0]\n```\n[c][0]", 1);
        assert_eq!(d.refs.len(), 1);
        assert_eq!(d.refs[0].line, 4);
    }

    #[test]
    fn headings_and_sections() {
        let d = doc(
            "intro\n## History [x][0]\ntext\n### Sub\nmore\n## Mission\nend",
            1,
        );
        let texts: Vec<_> = d.headings.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(texts, ["History x", "Sub", "Mission"]);
        assert_eq!(d.section_lines(0), 1..5);
        assert_eq!(d.section_lines(1), 3..5);
        assert_eq!(d.section_lines(2), 5..7);
        assert_eq!(d.heading_at(0), None);
        assert_eq!(d.heading_at(4), Some(1));
        assert_eq!(d.find_section("mission"), Some(2));
        assert_eq!(d.find_section("hist"), Some(0));
        assert_eq!(d.find_section("nope"), None);
    }

    #[test]
    fn anchors_match_headings() {
        let d = doc("## Re-entry and splashdown\n## Lunar flyby", 0);
        assert_eq!(d.find_anchor("Lunar_flyby"), Some(1));
        assert_eq!(d.find_anchor("re-entry-and-splashdown"), Some(0));
        assert_eq!(d.find_anchor("cite_note-1"), None);
    }

    #[test]
    fn resolves_links_against_page_url() {
        let links = vec![
            link(0, "/wiki/Moon"),
            link(1, "#History"),
            link(2, "https://o.test/"),
        ];
        let d = Doc::new("https://x.test/wiki/Artemis".into(), None, "", links);
        let hrefs: Vec<_> = d.links.iter().map(|l| l.href.as_str()).collect();
        assert_eq!(
            hrefs,
            [
                "https://x.test/wiki/Moon",
                "https://x.test/wiki/Artemis#History",
                "https://o.test/"
            ]
        );
    }

    #[test]
    fn markdown_for_appends_used_link_definitions() {
        let d = doc("## A\nsee [x][1] and [y][1]\n## B\n[z][0]", 2);
        assert_eq!(
            d.markdown_for(d.section_lines(0)),
            "## A\nsee [x][1] and [y][1]\n\n[1]: https://x.test/1"
        );
        assert_eq!(
            d.markdown_for(d.section_lines(1)),
            "## B\n[z][0]\n\n[0]: https://x.test/0"
        );
    }

    #[test]
    fn wraps_at_word_boundaries() {
        let d = doc("hello brave new world\n\nabcdefghij", 0);
        let rows: Vec<_> = d
            .wrap(10)
            .iter()
            .map(|r| &d.lines[r.line][r.start..r.end])
            .collect();
        assert_eq!(rows, ["hello ", "brave new ", "world", "", "abcdefghij"]);
        let hard: Vec<_> = d.wrap(4).iter().map(|r| (r.line, r.start, r.end)).collect();
        assert_eq!(hard[hard.len() - 3..], [(2, 0, 4), (2, 4, 8), (2, 8, 10)]);
    }

    #[test]
    fn wraps_wide_chars_by_display_width() {
        let d = doc("日本語テキスト", 0);
        let rows = d.wrap(4);
        assert_eq!(rows.len(), 4);
        assert_eq!(&d.lines[0][rows[0].start..rows[0].end], "日本");
    }

    #[test]
    fn styles_emphasis_links_and_code() {
        let d = doc("a **b** *c* `d*` [e][0] 5 * 3", 1);
        let line = d.lines[0].clone();
        let looks = d.line_looks(0);
        let at = |s: &str| looks[line.find(s).unwrap()];
        assert!(at("b").bold && !at("b").dim);
        assert!(at("**").dim);
        assert!(at("c").italic);
        assert!(at("d").code && !at("d").italic);
        assert!(at("e").link);
        assert!(at("[e").dim);
        assert!(!at("* 3").dim && !at("3").italic);
    }

    #[test]
    fn styles_block_markers() {
        let d = doc("## Title\n- item\n| a | b |\n|---|---|\n> quote\n---", 0);
        assert_eq!(d.line_looks(0)[0].heading, 2);
        assert!(d.line_looks(0)[0].dim && !d.line_looks(0)[3].dim);
        assert!(d.line_looks(1)[0].dim && !d.line_looks(1)[2].dim);
        assert!(d.line_looks(2)[0].dim && !d.line_looks(2)[2].dim);
        assert!(d.line_looks(3).iter().all(|l| l.dim));
        assert!(d.line_looks(4)[2].italic);
        assert!(d.line_looks(5).iter().all(|l| l.dim));
    }
}
