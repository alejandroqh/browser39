//! Full-screen terminal pager for `browse`.

use std::collections::HashMap;
use std::io::{self, Write};
use std::mem;
use std::ops::Range;

use anyhow::Result;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, ContentStyle, PrintStyledContent, StyledContent};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::Browser;
use super::clipboard;
use super::doc::{Doc, Look, Row};

const HELP: &[(&str, &str)] = &[
    ("j k ↓ ↑", "scroll one line"),
    ("space b PgDn PgUp", "scroll one page"),
    ("d u", "scroll half a page"),
    ("g G Home End", "top / bottom"),
    ("Tab Shift-Tab", "next / previous link"),
    ("Enter", "follow selected link"),
    ("12 Enter", "follow link [12]"),
    ("h ← Backspace", "back"),
    ("l →", "forward"),
    ("o", "open URL or search the web"),
    ("O", "edit current URL"),
    ("/ n N", "find text, next / previous match"),
    ("t", "table of contents (Enter jump, y copy)"),
    ("y", "copy current section as markdown"),
    ("Y", "copy whole page as markdown"),
    ("c", "copy link URL (or page URL)"),
    ("r", "toggle reader / full page"),
    ("Esc", "clear selection and search"),
    ("q Ctrl-C", "quit"),
];

enum Mode {
    Normal,
    Prompt { kind: PromptKind, input: String },
    Toc { sel: usize },
    Help,
}

#[derive(Clone, Copy)]
enum PromptKind {
    Open,
    Find,
}

#[derive(Clone, Copy)]
enum Overlay {
    None,
    Hit,
    CurrentHit,
    Selected,
}

/// Scroll position and link selection remembered per history entry.
#[derive(Clone, Copy, Default)]
struct ViewState {
    line: usize,
    selected: Option<usize>,
}

pub(super) async fn run(browser: Browser<'_>, doc: Doc, section: Option<&str>) -> Result<()> {
    let (w, h) = terminal::size()?;
    let _guard = TerminalGuard::enter()?;
    let mut ui = Ui::new(browser, doc, w, h);
    if let Some(name) = section {
        match ui.doc.find_section(name) {
            Some(h) => ui.jump_to_heading(h),
            None => ui.message = Some(format!("No section matching {name:?}")),
        }
    }
    loop {
        ui.draw()?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if !ui.on_key(key).await? {
                    return Ok(());
                }
            }
            Event::Resize(w, h) => ui.resize(w, h),
            _ => {}
        }
    }
}

struct Ui<'a> {
    browser: Browser<'a>,
    doc: Doc,
    rows: Vec<Row>,
    width: usize,
    height: usize,
    /// First visible row.
    top: usize,
    /// Selected link reference (index into `doc.refs`).
    selected: Option<usize>,
    find: Option<String>,
    /// Matches of `find` in document order.
    hits: Vec<(usize, Range<usize>)>,
    hit: Option<usize>,
    /// Digits typed to follow a link by number.
    number: String,
    mode: Mode,
    message: Option<String>,
    views: HashMap<usize, ViewState>,
}

impl<'a> Ui<'a> {
    fn new(browser: Browser<'a>, doc: Doc, w: u16, h: u16) -> Self {
        let mut ui = Self {
            browser,
            doc,
            rows: Vec::new(),
            width: 0,
            height: 0,
            top: 0,
            selected: None,
            find: None,
            hits: Vec::new(),
            hit: None,
            number: String::new(),
            mode: Mode::Normal,
            message: None,
            views: HashMap::new(),
        };
        ui.resize(w, h);
        ui
    }

    // --- Geometry ---

    fn content_height(&self) -> usize {
        self.height.saturating_sub(2).max(1)
    }

    fn max_top(&self) -> usize {
        self.rows.len().saturating_sub(self.content_height())
    }

    fn resize(&mut self, w: u16, h: u16) {
        let line = self.top_line();
        // One spare column avoids the terminal's pending-wrap quirks.
        self.width = (w as usize).saturating_sub(1).max(1);
        self.height = h as usize;
        self.rows = self.doc.wrap(self.width);
        self.top = self.row_of(line, 0).min(self.max_top());
    }

    fn top_line(&self) -> usize {
        self.rows.get(self.top).map_or(0, |r| r.line)
    }

    /// Display row containing byte `byte` of source line `line`.
    fn row_of(&self, line: usize, byte: usize) -> usize {
        self.rows
            .partition_point(|r| (r.line, r.start) <= (line, byte))
            .saturating_sub(1)
    }

    fn ref_row(&self, i: usize) -> usize {
        let r = &self.doc.refs[i];
        self.row_of(r.line, r.span.start)
    }

    fn is_visible(&self, row: usize) -> bool {
        row >= self.top && row < self.top + self.content_height()
    }

    /// Scroll so `row` is on screen, with some context above it.
    fn reveal(&mut self, row: usize) {
        if !self.is_visible(row) {
            self.top = row
                .saturating_sub(self.content_height() / 4)
                .min(self.max_top());
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        self.top = self.top.saturating_add_signed(delta).min(self.max_top());
        if let Some(i) = self.selected
            && !self.is_visible(self.ref_row(i))
        {
            self.selected = None;
        }
    }

    fn scroll_to(&mut self, row: usize) {
        self.top = row.min(self.max_top());
        self.selected = None;
    }

    fn jump_to_heading(&mut self, h: usize) {
        let row = self.row_of(self.doc.headings[h].line, 0);
        self.scroll_to(row);
    }

    /// Line that "current section" commands act on: the selected link, else a
    /// heading near the top of the screen, else the top line.
    fn anchor_line(&self) -> usize {
        if let Some(i) = self.selected {
            return self.doc.refs[i].line;
        }
        let near_top = self.top..(self.top + 3).min(self.rows.len());
        self.rows[near_top]
            .iter()
            .map(|r| r.line)
            .find(|&l| self.doc.headings.iter().any(|h| h.line == l))
            .unwrap_or_else(|| self.top_line())
    }

    // --- Documents and history ---

    fn show(&mut self, doc: Doc, view: ViewState) {
        self.doc = doc;
        self.rows = self.doc.wrap(self.width);
        self.top = self.row_of(view.line, 0).min(self.max_top());
        self.selected = view.selected.filter(|&i| i < self.doc.refs.len());
        self.number.clear();
        self.hits = self
            .find
            .as_deref()
            .map_or_else(Vec::new, |q| find_hits(&self.doc, q));
        self.hit = None;
    }

    fn save_view(&mut self) {
        let view = ViewState {
            line: self.top_line(),
            selected: self.selected,
        };
        self.views.insert(self.browser.history_index(), view);
    }

    async fn navigate(&mut self, url: String) -> Result<()> {
        self.save_view();
        self.message = Some(format!("Loading {url} …"));
        self.draw()?;
        match self.browser.open(&url).await {
            Ok(doc) => {
                let idx = self.browser.history_index();
                self.views.retain(|&k, _| k < idx);
                self.show(doc, ViewState::default());
                self.message = None;
            }
            Err(e) => self.message = Some(format!("Error: {e:#}")),
        }
        Ok(())
    }

    fn history_step(&mut self, back: bool) {
        self.save_view();
        match self.browser.step(back) {
            Ok(doc) => {
                let idx = self.browser.history_index();
                let view = self.views.get(&idx).copied().unwrap_or_default();
                self.show(doc, view);
            }
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    async fn follow(&mut self, link: usize) -> Result<()> {
        let href = self.doc.links[link].href.clone();
        if href.is_empty() {
            self.message = Some(format!("Link [{link}] has no URL"));
            return Ok(());
        }
        // Same-page anchors jump to the matching heading instead of refetching
        let page = self.doc.url.split('#').next().unwrap_or_default();
        if let Some((base, fragment)) = href.split_once('#')
            && base == page
        {
            match self.doc.find_anchor(fragment) {
                Some(h) => self.jump_to_heading(h),
                None => self.message = Some(format!("#{fragment} is not a section on this page")),
            }
            return Ok(());
        }
        self.navigate(href).await
    }

    fn toggle_reader(&mut self) {
        let heading = self
            .doc
            .heading_at(self.top_line())
            .map(|h| self.doc.headings[h].text.clone());
        self.browser.reader = !self.browser.reader;
        match self.browser.render() {
            Ok(doc) => {
                self.show(doc, ViewState::default());
                if let Some(h) =
                    heading.and_then(|t| self.doc.headings.iter().position(|h| h.text == t))
                {
                    self.jump_to_heading(h);
                }
                self.message = Some(if self.browser.reader {
                    "Reader mode: main content only".into()
                } else {
                    "Full page".into()
                });
            }
            Err(e) => self.message = Some(format!("Error: {e:#}")),
        }
    }

    // --- Links, search, copy ---

    fn step_link(&mut self, forward: bool) {
        let refs = &self.doc.refs;
        if refs.is_empty() {
            self.message = Some("No links on this page".into());
            return;
        }
        let bottom = self.top + self.content_height();
        let next = match self.selected {
            Some(i) if forward => (i + 1 < refs.len()).then_some(i + 1),
            Some(i) => i.checked_sub(1),
            None if forward => {
                let i = refs.partition_point(|r| self.row_of(r.line, r.span.start) < self.top);
                (i < refs.len()).then_some(i)
            }
            None => refs
                .partition_point(|r| self.row_of(r.line, r.span.start) < bottom)
                .checked_sub(1),
        };
        match next {
            Some(i) => {
                self.selected = Some(i);
                self.reveal(self.ref_row(i));
            }
            None => self.message = Some(if forward { "Last link" } else { "First link" }.into()),
        }
    }

    fn preview_number(&mut self) {
        let Ok(n) = self.number.parse::<usize>() else {
            return;
        };
        let Some(link) = self.doc.links.get(n) else {
            self.message = Some(format!("No link [{n}]"));
            return;
        };
        self.message = Some(format!("[{n}] {} — Enter to follow", link.href));
        // Highlight an on-screen occurrence, but never scroll: partial numbers
        // ("1" on the way to "17") would otherwise yank the view around.
        self.selected = (0..self.doc.refs.len())
            .find(|&i| self.doc.refs[i].link == n && self.is_visible(self.ref_row(i)));
    }

    fn start_find(&mut self, query: String) {
        if !query.is_empty() {
            self.find = Some(query);
        }
        let Some(q) = self.find.as_deref() else {
            return;
        };
        self.hits = find_hits(&self.doc, q);
        self.hit = None;
        self.next_hit(true);
    }

    fn next_hit(&mut self, forward: bool) {
        let Some(q) = self.find.clone() else {
            self.message = Some("No search yet — press /".into());
            return;
        };
        if self.hits.is_empty() {
            self.message = Some(format!("Not found: {q}"));
            return;
        }
        let n = self.hits.len();
        let k = match self.hit {
            Some(k) if forward => (k + 1) % n,
            Some(k) => (k + n - 1) % n,
            None => {
                let first = self
                    .hits
                    .partition_point(|(l, r)| self.row_of(*l, r.start) < self.top);
                if forward {
                    first % n
                } else {
                    (first + n - 1) % n
                }
            }
        };
        self.hit = Some(k);
        let (line, ref r) = self.hits[k];
        self.reveal(self.row_of(line, r.start));
        self.message = Some(format!("/{q}  match {} of {n}", k + 1));
    }

    fn copy(&mut self, what: &str, text: &str) {
        self.message = Some(match clipboard::copy(text) {
            Ok(via) => format!(
                "Copied {what} — {} lines, {} (via {via})",
                text.lines().count(),
                human_size(text.len())
            ),
            Err(e) => format!("Copy failed: {e}"),
        });
    }

    fn copy_section(&mut self, h: Option<usize>) {
        let (what, lines) = match h {
            Some(h) => (
                format!("section “{}”", self.doc.headings[h].text),
                self.doc.section_lines(h),
            ),
            None => match self.doc.headings.first() {
                Some(first) if first.line > 0 => ("page intro".into(), 0..first.line),
                _ => ("page".into(), 0..self.doc.lines.len()),
            },
        };
        let text = self.doc.markdown_for(lines);
        self.copy(&what, &text);
    }

    // --- Keys ---

    /// Handle a key press. Returns `false` to quit.
    async fn on_key(&mut self, key: KeyEvent) -> Result<bool> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Ok(false);
        }
        match &mut self.mode {
            Mode::Normal => return self.on_normal_key(key, ctrl).await,
            Mode::Help => self.mode = Mode::Normal,
            Mode::Toc { sel } => {
                let sel = *sel;
                self.on_toc_key(key, sel);
            }
            Mode::Prompt { kind, input } => match key.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Enter => {
                    let (kind, input) = (*kind, mem::take(input));
                    self.mode = Mode::Normal;
                    self.submit(kind, input).await?;
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char('u') if ctrl => input.clear(),
                KeyCode::Char(c) => input.push(c),
                _ => {}
            },
        }
        Ok(true)
    }

    async fn submit(&mut self, kind: PromptKind, input: String) -> Result<()> {
        match kind {
            PromptKind::Find => self.start_find(input),
            PromptKind::Open if input.trim().is_empty() => {}
            PromptKind::Open => {
                let url = self.browser.resolve(&input);
                self.navigate(url).await?;
            }
        }
        Ok(())
    }

    async fn on_normal_key(&mut self, key: KeyEvent, ctrl: bool) -> Result<bool> {
        let page = self.content_height().saturating_sub(2).max(1) as isize;
        let half = (self.content_height() / 2).max(1) as isize;
        let typing_number = matches!(key.code, KeyCode::Char('0'..='9'))
            || (!self.number.is_empty() && matches!(key.code, KeyCode::Enter | KeyCode::Backspace));
        if !typing_number {
            self.number.clear();
        }
        self.message = None;

        match key.code {
            KeyCode::Char('q') => return Ok(false),
            KeyCode::Esc => {
                self.selected = None;
                self.find = None;
                self.hits.clear();
                self.hit = None;
            }
            KeyCode::Char(c @ '0'..='9') => {
                self.number.push(c);
                self.preview_number();
            }
            KeyCode::Backspace if !self.number.is_empty() => {
                self.number.pop();
                self.preview_number();
            }
            KeyCode::Enter if !self.number.is_empty() => {
                let n = mem::take(&mut self.number)
                    .parse::<usize>()
                    .unwrap_or(usize::MAX);
                if n < self.doc.links.len() {
                    self.follow(n).await?;
                } else {
                    self.message = Some("No such link".into());
                }
            }
            KeyCode::Enter => match self.selected {
                Some(i) => self.follow(self.doc.refs[i].link).await?,
                None => {
                    self.message =
                        Some("No link selected — Tab to select, or type its number".into())
                }
            },
            KeyCode::Tab => self.step_link(true),
            KeyCode::BackTab => self.step_link(false),

            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(page),
            KeyCode::Char('f') if ctrl => self.scroll_by(page),
            KeyCode::PageUp | KeyCode::Char('b') => self.scroll_by(-page),
            KeyCode::Char('d') => self.scroll_by(half),
            KeyCode::Char('u') => self.scroll_by(-half),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_to(0),
            KeyCode::End | KeyCode::Char('G') => self.scroll_to(self.max_top()),

            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => self.history_step(true),
            KeyCode::Right | KeyCode::Char('l') => self.history_step(false),

            KeyCode::Char('o') => {
                self.mode = Mode::Prompt {
                    kind: PromptKind::Open,
                    input: String::new(),
                }
            }
            KeyCode::Char('O') => {
                self.mode = Mode::Prompt {
                    kind: PromptKind::Open,
                    input: self.doc.url.clone(),
                }
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Prompt {
                    kind: PromptKind::Find,
                    input: String::new(),
                }
            }
            KeyCode::Char('n') => self.next_hit(true),
            KeyCode::Char('N') => self.next_hit(false),

            KeyCode::Char('t') if self.doc.headings.is_empty() => {
                self.message = Some("No headings on this page".into())
            }
            KeyCode::Char('t') => {
                let sel = self.doc.heading_at(self.anchor_line()).unwrap_or(0);
                self.mode = Mode::Toc { sel };
            }
            KeyCode::Char('y') => self.copy_section(self.doc.heading_at(self.anchor_line())),
            KeyCode::Char('Y') => {
                let text = self.doc.markdown_for(0..self.doc.lines.len());
                self.copy("page", &text);
            }
            KeyCode::Char('c') => {
                let (what, url) = match self.selected {
                    Some(i) => (
                        "link URL",
                        self.doc.links[self.doc.refs[i].link].href.clone(),
                    ),
                    None => ("page URL", self.doc.url.clone()),
                };
                self.copy(what, &url);
            }
            KeyCode::Char('r') => self.toggle_reader(),
            KeyCode::Char('?') => self.mode = Mode::Help,
            _ => {}
        }
        Ok(true)
    }

    fn on_toc_key(&mut self, key: KeyEvent, sel: usize) {
        let last = self.doc.headings.len().saturating_sub(1);
        let page = self.content_height().saturating_sub(1).max(1);
        let sel = match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.jump_to_heading(sel);
                return;
            }
            KeyCode::Char('y') => {
                self.copy_section(Some(sel));
                sel
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => (sel + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => sel.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => (sel + page).min(last),
            KeyCode::PageUp | KeyCode::Char('b') => sel.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => last,
            _ => sel,
        };
        self.mode = Mode::Toc { sel };
    }

    // --- Drawing ---

    fn draw(&self) -> io::Result<()> {
        let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
        queue!(buf, BeginSynchronizedUpdate, Hide)?;

        let title = self.doc.title.as_deref().unwrap_or("");
        let bar = format!("{title}  {}", self.doc.url);
        queue!(buf, MoveTo(0, 0))?;
        print_bar(&mut buf, &bar, "", self.width + 1)?;

        match &self.mode {
            Mode::Toc { sel } => self.draw_toc(&mut buf, *sel)?,
            Mode::Help => self.draw_help(&mut buf)?,
            _ => self.draw_page(&mut buf)?,
        }

        let status_y = self.height.saturating_sub(1) as u16;
        queue!(buf, MoveTo(0, status_y))?;
        let cursor = self.draw_status(&mut buf)?;
        if let Some(col) = cursor {
            queue!(buf, MoveTo(col, status_y), Show)?;
        }
        queue!(buf, EndSynchronizedUpdate)?;

        let mut out = io::stdout().lock();
        out.write_all(&buf)?;
        out.flush()
    }

    fn draw_page(&self, buf: &mut Vec<u8>) -> io::Result<()> {
        let mut cached: Option<(usize, Vec<Look>, Vec<Overlay>)> = None;
        for y in 0..self.content_height() {
            queue!(buf, MoveTo(0, (y + 1) as u16))?;
            if let Some(row) = self.rows.get(self.top + y) {
                if cached.as_ref().is_none_or(|(l, ..)| *l != row.line) {
                    cached = Some((
                        row.line,
                        self.doc.line_looks(row.line),
                        self.overlays(row.line),
                    ));
                }
                let (_, looks, overlays) = cached.as_ref().expect("cached above");
                let line = &self.doc.lines[row.line];
                let mut seg = String::new();
                let mut seg_style = None;
                for (i, ch) in line[row.start..row.end].char_indices() {
                    let b = row.start + i;
                    let style = style_for(looks[b], overlays[b]);
                    if seg_style != Some(style) {
                        if let Some(s) = seg_style {
                            queue!(
                                buf,
                                PrintStyledContent(StyledContent::new(s, mem::take(&mut seg)))
                            )?;
                        }
                        seg_style = Some(style);
                    }
                    seg.push(ch);
                }
                if let Some(s) = seg_style {
                    queue!(buf, PrintStyledContent(StyledContent::new(s, seg)))?;
                }
            } else {
                queue!(buf, PrintStyledContent(StyledContent::new(dim(), "~")))?;
            }
            queue!(buf, Clear(ClearType::UntilNewLine))?;
        }
        Ok(())
    }

    fn overlays(&self, line: usize) -> Vec<Overlay> {
        let mut ov = vec![Overlay::None; self.doc.lines[line].len()];
        let start = self.hits.partition_point(|(l, _)| *l < line);
        for (k, (l, r)) in self.hits.iter().enumerate().skip(start) {
            if *l != line {
                break;
            }
            let kind = if self.hit == Some(k) {
                Overlay::CurrentHit
            } else {
                Overlay::Hit
            };
            ov[r.clone()].fill(kind);
        }
        if let Some(i) = self.selected
            && self.doc.refs[i].line == line
        {
            ov[self.doc.refs[i].span.clone()].fill(Overlay::Selected);
        }
        ov
    }

    fn draw_toc(&self, buf: &mut Vec<u8>, sel: usize) -> io::Result<()> {
        let headings = &self.doc.headings;
        let ch = self.content_height();
        let min_level = headings.iter().map(|h| h.level).min().unwrap_or(1);
        let top = sel
            .saturating_sub(ch.saturating_sub(1) / 2)
            .min(headings.len().saturating_sub(ch));
        for y in 0..ch {
            queue!(buf, MoveTo(0, (y + 1) as u16))?;
            if let Some(h) = headings.get(top + y) {
                let indent = "  ".repeat(h.level - min_level);
                let text = fit(&format!(" {indent}{}", h.text), self.width);
                let mut style = heading_style(h.level as u8);
                if top + y == sel {
                    style.attributes.set(Attribute::Reverse);
                }
                queue!(buf, PrintStyledContent(StyledContent::new(style, text)))?;
            }
            queue!(buf, Clear(ClearType::UntilNewLine))?;
        }
        Ok(())
    }

    fn draw_help(&self, buf: &mut Vec<u8>) -> io::Result<()> {
        let mut lines = vec![
            String::from(" Keys — press any key to return"),
            String::new(),
        ];
        lines.extend(HELP.iter().map(|(k, d)| format!(" {k:>18}   {d}")));
        for y in 0..self.content_height() {
            queue!(buf, MoveTo(0, (y + 1) as u16))?;
            if let Some(l) = lines.get(y) {
                let style = if y == 0 {
                    heading_style(2)
                } else {
                    ContentStyle::new()
                };
                queue!(
                    buf,
                    PrintStyledContent(StyledContent::new(style, fit(l, self.width)))
                )?;
            }
            queue!(buf, Clear(ClearType::UntilNewLine))?;
        }
        Ok(())
    }

    /// Draw the status line. Returns the cursor column when a prompt is open.
    fn draw_status(&self, buf: &mut Vec<u8>) -> io::Result<Option<u16>> {
        let full = self.width + 1;
        match &self.mode {
            Mode::Prompt { kind, input } => {
                let label = match kind {
                    PromptKind::Open => "Open URL or search: ",
                    PromptKind::Find => "/",
                };
                // Keep the end of long input (and the cursor) visible
                let room = full.saturating_sub(label.len() + 2);
                let shown = tail(input, room);
                let text = format!("{label}{shown}");
                print_bar(buf, &text, "", full)?;
                Ok(Some((1 + text.width()).min(full.saturating_sub(1)) as u16))
            }
            Mode::Toc { .. } => {
                print_bar(
                    buf,
                    "Contents — Enter jump · y copy section · Esc close",
                    "",
                    full,
                )?;
                Ok(None)
            }
            _ => {
                let left = if let Some(m) = &self.message {
                    m.clone()
                } else if let Some(i) = self.selected {
                    let n = self.doc.refs[i].link;
                    format!("[{n}] {}", self.doc.links[n].href)
                } else {
                    "? help · Tab links · t contents · y copy section · o open · q quit".into()
                };
                print_bar(buf, &left, &self.position_label(), full)?;
                Ok(None)
            }
        }
    }

    fn position_label(&self) -> String {
        let section = self
            .doc
            .heading_at(self.anchor_line())
            .map(|h| format!("§ {} · ", fit(&self.doc.headings[h].text, 30)))
            .unwrap_or_default();
        let mode = if self.browser.reader {
            "reader"
        } else {
            "full"
        };
        let ch = self.content_height();
        let pos = if self.rows.len() <= ch {
            "All".to_string()
        } else if self.top == 0 {
            "Top".to_string()
        } else if self.top >= self.max_top() {
            "End".to_string()
        } else {
            format!("{}%", (self.top + ch) * 100 / self.rows.len())
        };
        format!(" {section}{mode} · {pos} ")
    }
}

/// Every case-insensitive (ASCII) match of `query`, in document order.
fn find_hits(doc: &Doc, query: &str) -> Vec<(usize, Range<usize>)> {
    let q = query.to_ascii_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (i, line) in doc.lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        hits.extend(lower.match_indices(&q).map(|(s, m)| (i, s..s + m.len())));
    }
    hits
}

// --- Styling helpers ---

fn style_for(look: Look, overlay: Overlay) -> ContentStyle {
    let mut s = if look.heading > 0 {
        heading_style(look.heading)
    } else {
        ContentStyle::new()
    };
    if look.link {
        s.foreground_color = Some(Color::Cyan);
        s.attributes.set(Attribute::Underlined);
    } else if look.code {
        s.foreground_color = Some(Color::Red);
    }
    if look.bold {
        s.attributes.set(Attribute::Bold);
    }
    if look.italic {
        s.attributes.set(Attribute::Italic);
    }
    if look.dim {
        s.foreground_color = Some(Color::DarkGrey);
        s.attributes.unset(Attribute::Underlined);
    }
    match overlay {
        Overlay::None => {}
        Overlay::Selected => {
            s.attributes.set(Attribute::Reverse);
            s.attributes.set(Attribute::Bold);
        }
        Overlay::Hit => {
            s.background_color = Some(Color::DarkYellow);
            s.foreground_color = Some(Color::Black);
        }
        Overlay::CurrentHit => {
            s.background_color = Some(Color::Yellow);
            s.foreground_color = Some(Color::Black);
            s.attributes.set(Attribute::Bold);
        }
    }
    s
}

fn heading_style(level: u8) -> ContentStyle {
    let mut s = ContentStyle::new();
    s.foreground_color = Some(match level {
        1 => Color::Magenta,
        2 => Color::Yellow,
        _ => Color::Green,
    });
    s.attributes.set(Attribute::Bold);
    s
}

fn dim() -> ContentStyle {
    let mut s = ContentStyle::new();
    s.foreground_color = Some(Color::DarkGrey);
    s
}

/// Print `left` and right-aligned `right` across `width` columns.
/// Print a reverse-video bar across the full `width`: `left` after a one-space
/// margin, `right` right-aligned.
fn print_bar(buf: &mut Vec<u8>, left: &str, right: &str, width: usize) -> io::Result<()> {
    let right = fit(right, width / 2);
    let right_w = right.width();
    let left = fit(left, width.saturating_sub(right_w + 2));
    let gap = width.saturating_sub(1 + left.width() + right_w);
    let mut style = ContentStyle::new();
    style.attributes.set(Attribute::Reverse);
    // Filling the last column is fine; a following clear would erase it.
    let line = format!(" {left}{}{right}", " ".repeat(gap));
    queue!(buf, PrintStyledContent(StyledContent::new(style, line)))
}

/// Truncate `s` to `width` display columns, ending with `…` when cut.
fn fit(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.push('…');
    out
}

/// The last `width` display columns of `s`.
fn tail(s: &str, width: usize) -> &str {
    let mut w = 0;
    for (i, c) in s.char_indices().rev() {
        w += c.width().unwrap_or(0);
        if w > width {
            return &s[i + c.len_utf8()..];
        }
    }
    s
}

fn human_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// Raw mode + alternate screen for the viewer's lifetime; restored on drop
/// and on panic (release builds abort on panic, so Drop alone isn't enough).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            default_hook(info);
        }));
        let guard = Self;
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}
